use bytes::Bytes;
use ecsdb::EntityId;
use http::{StatusCode, header};

use http_body_util::BodyExt as _;
use iref::iri;
use serde::Deserialize;

use futures_util::TryStreamExt;
use http::Method;
use tracing::{debug, instrument};

use crate::{LastAccess, pages};

use super::ResponseBody;

pub async fn ecs_service<RB, DbFun>(
    base_url: http::Uri,
    open_db: DbFun,
    request: http::Request<RB>,
) -> Result<http::Response<ResponseBody>, Error>
where
    DbFun: Fn(&http::Request<RB>) -> Result<ecsdb::Ecs, ecsdb::Error>,
    RB: http_body::Body<Data = Bytes> + Unpin,
    RB::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let is_hx_request = request
        .headers()
        .get("HX-Request")
        .is_some_and(|h| h.to_str().is_ok_and(|v| v == "true"));

    let is_hx_boosted = request
        .headers()
        .get("HX-Boosted")
        .is_some_and(|h| h.to_str().is_ok_and(|v| v == "true"));

    let is_htmx_request = is_hx_request && !is_hx_boosted;

    let db = open_db(&request)?;
    let kind = RequestType::from_request(request).await?;

    let wrap_markup = |markup| {
        if is_htmx_request {
            markup
        } else {
            let breadcrumbs = super::Breadcrumb::from_request(&kind);
            pages::wrap_in_body(&base_url, &breadcrumbs, markup)
        }
    };

    match kind.handle(db).await? {
        EcsResponse::Markup(markup) => {
            let markup = wrap_markup(markup);

            http::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html")
                .body(ResponseBody::from(markup.into_string()))
        }
        EcsResponse::Redirect(path) => {
            // Prepent base url to our redirect target
            let mut base = iri::PathBuf::new(base_url.path().to_owned()).unwrap();
            base.symbolic_append(path.segments());

            http::Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header(header::LOCATION, base.as_str())
                .body(ResponseBody::new(Bytes::new()))
        }
        EcsResponse::NotFound => {
            let markup = wrap_markup(pages::not_found());
            http::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(ResponseBody::new(Bytes::from(markup.into_string())))
        }
        EcsResponse::Download {
            filename,
            data,
            content_type,
        } => http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type.to_string())
            .header(
                header::CONTENT_DISPOSITION,
                format!(r#"attachment; filename="{filename}""#),
            )
            .header(header::CONTENT_LENGTH, data.len())
            .body(ResponseBody::new(Bytes::from(data))),
    }
    .map_err(|e| Error::Other(Box::new(e)))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid request '{0} {1}")]
    InvalidRequest(Method, String),

    #[error("Invalid EntityId {0:?}")]
    InvalidEntityId(String),

    #[error(transparent)]
    Ecs(#[from] ecsdb::Error),

    #[error("Invalid component data {0:?}")]
    InvalidComponentData(String),

    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    pub fn into_response(self) -> http::Response<ResponseBody> {
        let status = match &self {
            Error::InvalidRequest(_, _) => StatusCode::BAD_REQUEST,
            Error::InvalidEntityId(_) => StatusCode::BAD_REQUEST,
            Error::Ecs(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Error::InvalidComponentData(_) => StatusCode::BAD_REQUEST,
            Error::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        http::Response::builder()
            .status(status)
            .body(self.to_string().into())
            .unwrap()
    }
}

pub enum RequestType {
    Entities,
    Entity(EntityId),
    Component {
        entity_id: EntityId,
        component: String,
    },
    ModifyComponent {
        entity_id: EntityId,
        component: String,
        value: serde_json::Value,
    },
    DownloadComponent {
        entity: EntityId,
        component: String,
    },
}

enum EcsResponse {
    Markup(maud::Markup),
    Redirect(iri::PathBuf),
    NotFound,
    Download {
        filename: String,
        content_type: mime::Mime,
        data: Vec<u8>,
    },
}

impl RequestType {
    #[instrument(level = "debug", ret, skip_all, fields(request.url = %req.uri()))]
    async fn from_request<RB>(req: http::Request<RB>) -> Result<Self, Error>
    where
        RB: http_body::Body<Data = Bytes> + Unpin,
        RB::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let url = url::Url::parse("http://localhost")
            .unwrap()
            .join(&req.uri().to_string())
            .unwrap();

        let path_components: Box<[&str]> =
            url.path_segments().map(|s| s.collect()).unwrap_or_default();

        debug!(?path_components);

        match (req.method(), path_components.iter().as_slice()) {
            (&Method::GET, &["entities"]) => Ok(Self::Entities),

            (&Method::GET, &["entities", entity_id]) => {
                let Ok(entity_id) = str::parse::<EntityId>(entity_id) else {
                    return Err(Error::InvalidEntityId(entity_id.into()));
                };

                Ok(Self::Entity(entity_id))
            }

            (&Method::GET, &["entities", entity_id, "components", component]) => {
                let Ok(entity_id) = str::parse::<EntityId>(entity_id) else {
                    return Err(Error::InvalidEntityId(entity_id.into()));
                };

                Ok(Self::Component {
                    entity_id,
                    component: component.to_owned(),
                })
            }

            (&Method::GET, &["entities", entity_id, "components", component, "download"]) => {
                let Ok(entity) = str::parse::<EntityId>(entity_id) else {
                    return Err(Error::InvalidEntityId(entity_id.into()));
                };

                Ok(Self::DownloadComponent {
                    entity,
                    component: component.to_owned(),
                })
            }

            (&Method::POST, &["entities", entity_id, "components", component]) => {
                #[derive(Deserialize)]
                struct FormData {
                    #[serde(rename = "component_data")]
                    value: serde_json::Value,
                }

                let Ok(entity_id) = str::parse::<EntityId>(entity_id) else {
                    return Err(Error::InvalidEntityId(entity_id.into()));
                };

                let (_req, body) = req.into_parts();

                let form_data = tokio::task::block_in_place(|| {
                    let byte_stream = body
                        .into_data_stream()
                        .map_err(|e| std::io::Error::other(e.into()));

                    let reader = tokio_util::io::StreamReader::new(byte_stream);
                    let sync_reader = tokio_util::io::SyncIoBridge::new(reader);
                    serde_urlencoded::from_reader::<FormData, _>(sync_reader)
                });

                match form_data {
                    Ok(FormData { value }) => Ok(Self::ModifyComponent {
                        entity_id,
                        component: component.to_owned(),
                        value,
                    }),
                    Err(e) => Err(Error::InvalidComponentData(e.to_string())),
                }
            }

            (method, path) => Err(Error::InvalidRequest(method.to_owned(), path.join("/"))),
        }
    }

    #[instrument(level = "debug", skip(db), ret)]
    async fn handle(&self, db: ecsdb::Ecs) -> Result<EcsResponse, Error> {
        match *self {
            RequestType::Entities => {
                let entities = db.query::<ecsdb::Entity, ()>();
                Ok(EcsResponse::Markup(pages::entities(entities)))
            }
            RequestType::Entity(eid) => {
                let Some(entity) = db.find(eid).next() else {
                    return Ok(EcsResponse::NotFound);
                };

                entity.attach(LastAccess::now());
                Ok(EcsResponse::Markup(pages::entity(entity)))
            }
            RequestType::Component {
                entity_id: entity,
                ref component,
            } => {
                let Some(entity) = db.find(entity).next() else {
                    return Ok(EcsResponse::NotFound);
                };

                entity.attach(LastAccess::now());
                Ok(EcsResponse::Markup(pages::component_editor(
                    entity, component,
                )))
            }
            RequestType::ModifyComponent {
                entity_id,
                ref component,
                ref value,
            } => {
                let Some(entity) = db.find(entity_id).next() else {
                    return Ok(EcsResponse::NotFound);
                };

                let target =
                    iri::PathBuf::new(format!("entities/{entity_id}/components/{component}"))
                        .unwrap();

                let component = match ecsdb::DynComponent::from_json(component, value) {
                    Ok(c) => c,
                    Err(e) => todo!("{e:?}"),
                };

                entity.attach(LastAccess::now()).dyn_attach(component);

                Ok(EcsResponse::Redirect(target))
            }
            Self::DownloadComponent {
                entity,
                ref component,
            } => {
                let Some(entity) = db.find(entity).next() else {
                    return Ok(EcsResponse::Markup(pages::not_found()));
                };

                let Some(component) = entity.dyn_component(component) else {
                    return Ok(EcsResponse::NotFound);
                };

                let filename = {
                    let ext = match component.kind() {
                        Kind::Json => ".json",
                        Kind::Blob | Kind::Null | Kind::Other(_) => "",
                    };

                    format!(
                        "ecsdb_{}_{}{}",
                        entity.id(),
                        component.name().replace("::", "-"),
                        ext
                    )
                };

                use ecsdb::dyn_component::Kind;
                let content_type = match component.kind() {
                    Kind::Json => mime::APPLICATION_JSON,
                    Kind::Blob | Kind::Null | Kind::Other(_) => mime::APPLICATION_OCTET_STREAM,
                };

                let Some(data) = component.into_blob() else {
                    return Ok(EcsResponse::NotFound);
                };

                entity.attach(LastAccess::now());

                Ok(EcsResponse::Download {
                    filename,
                    content_type,
                    data,
                })
            }
        }
    }
}

impl std::fmt::Debug for EcsResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Markup(_markup) => f.debug_tuple("Markup").field(&"<html>").finish(),
            Self::Redirect(path) => f.debug_tuple("Redirect").field(path).finish(),
            Self::NotFound => f.debug_tuple("NotFound").finish(),
            Self::Download {
                content_type,
                data,
                filename,
            } => f
                .debug_tuple("Download")
                .field(filename)
                .field(content_type)
                .field(&format_args!("{}b", data.len()))
                .finish(),
        }
    }
}

impl std::fmt::Debug for RequestType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entities => write!(f, "Entities"),
            Self::Entity(arg0) => f.debug_tuple("Entity").field(arg0).finish(),
            Self::Component {
                entity_id,
                component,
            } => f
                .debug_tuple("Component")
                .field(entity_id)
                .field(&format_args!("{component}"))
                .finish(),
            Self::ModifyComponent {
                entity_id,
                component,
                value: _,
            } => f
                .debug_tuple("ModifyComponent")
                .field(entity_id)
                .field(&format_args!("{component}"))
                .field(&format_args!("<redacted>"))
                .finish(),
            Self::DownloadComponent { entity, component } => f
                .debug_tuple("Download")
                .field(entity)
                .field(component)
                .finish(),
        }
    }
}
