use std::{collections::HashSet, fmt::Debug, marker::PhantomData, str::FromStr};

use bytes::Bytes;
use ecsdb::EntityId;
use http::{StatusCode, header};

use http_body_util::BodyExt as _;
use iref::iri;
use serde::{Deserialize, Serialize};

use futures_util::TryStreamExt;
use http::Method;
use tracing::{debug, instrument};
use url::form_urlencoded;

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
    let kind = Request::from_request(request).await?;

    let wrap_markup = |markup| {
        if is_htmx_request {
            markup
        } else {
            let breadcrumbs = super::Breadcrumb::from_request(&kind);
            pages::wrap_in_body(&base_url, &breadcrumbs, markup)
        }
    };

    match kind.handle(db).await? {
        Response::Markup(markup) => {
            let markup = wrap_markup(markup);

            http::Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(ResponseBody::from(markup.into_string()))
        }
        Response::Redirect(path) => {
            // Prepend base url to our redirect target
            let base_url_without_trailing_slash = base_url.path().trim_end_matches('/');
            let mut base = iri::PathBuf::new(base_url_without_trailing_slash.to_owned()).unwrap();
            base.symbolic_append(path.segments());

            http::Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header(header::LOCATION, base.as_str())
                .body(ResponseBody::new(Bytes::new()))
        }
        Response::NotFound => {
            let markup = wrap_markup(pages::not_found());
            http::Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(ResponseBody::new(Bytes::from(markup.into_string())))
        }
        Response::Download {
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

#[derive(Debug, Deserialize, Clone, Serialize)]
pub(crate) struct Filter {
    #[serde(
        deserialize_with = "deserialize_comma_separated_string",
        serialize_with = "serialize_comma_separated_string",
        default
    )]
    pub component_names: HashSet<String>,

    #[serde(default = "zero_entity_id")]
    pub after: EntityId,

    #[serde(default = "default_per_page")]
    pub count: usize,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            component_names: Default::default(),
            after: zero_entity_id(),
            count: default_per_page(),
        }
    }
}

fn default_per_page() -> usize {
    20
}

fn zero_entity_id() -> EntityId {
    0
}

pub enum Request {
    Index,
    Entities {
        filter: Filter,
    },
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
        entity_id: EntityId,
        component: String,
    },
}

enum Response {
    Markup(maud::Markup),
    Redirect(iri::PathBuf),
    NotFound,
    Download {
        filename: String,
        content_type: mime::Mime,
        data: Vec<u8>,
    },
}

impl Request {
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
            (&Method::GET, &[] | &[""]) => Ok(Self::Index),
            (&Method::GET, &["entities"]) => {
                let query = url.query().unwrap_or_default();
                let filter: Filter =
                    serde_urlencoded::from_str(query).map_err(|e| Error::Other(Box::new(e)))?;
                Ok(Self::Entities { filter })
            }

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
                let Ok(entity_id) = str::parse::<EntityId>(entity_id) else {
                    return Err(Error::InvalidEntityId(entity_id.into()));
                };

                Ok(Self::DownloadComponent {
                    entity_id,
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
    async fn handle(&self, db: ecsdb::Ecs) -> Result<Response, Error> {
        match self {
            Self::Index => Ok(Response::Redirect(
                iri::PathBuf::from_str("entities").unwrap(),
            )),
            Self::Entities { filter } => {
                let mut entities = db
                    .query::<ecsdb::Entity, ()>()
                    .filter(|e| e.id() > filter.after)
                    .filter(|e| {
                        filter.component_names.is_empty()
                            || filter
                                .component_names
                                .is_subset(&e.component_names().collect())
                    })
                    .take(filter.count)
                    .collect::<Vec<_>>();
                entities.sort_by_key(|e| e.id());

                let next_page = Filter {
                    after: entities.last().map(ecsdb::Entity::id).unwrap_or_default(),
                    ..filter.clone()
                };

                let all_component_names = db.component_names()?;

                Ok(Response::Markup(pages::entities(
                    &entities,
                    &next_page,
                    &all_component_names,
                )))
            }
            Self::Entity(eid) => {
                let Some(entity) = db.find(*eid).next() else {
                    return Ok(Response::NotFound);
                };

                entity.attach(LastAccess::now());
                Ok(Response::Markup(pages::entity(entity)))
            }
            Self::Component {
                entity_id,
                component,
            } => {
                let Some(entity) = db.find(*entity_id).next() else {
                    return Ok(Response::NotFound);
                };

                entity.attach(LastAccess::now());
                Ok(Response::Markup(pages::component_editor(entity, component)))
            }
            Self::ModifyComponent {
                entity_id,
                component,
                value,
            } => {
                let Some(entity) = db.find(*entity_id).next() else {
                    return Ok(Response::NotFound);
                };

                let target =
                    iri::PathBuf::new(format!("entities/{entity_id}/components/{component}"))
                        .unwrap();

                let component = match ecsdb::DynComponent::from_json(component, value) {
                    Ok(c) => c,
                    Err(e) => todo!("{e:?}"),
                };

                entity.attach(LastAccess::now()).dyn_attach(component);

                Ok(Response::Redirect(target))
            }
            Self::DownloadComponent {
                entity_id,
                component,
            } => {
                let Some(entity) = db.find(*entity_id).next() else {
                    return Ok(Response::Markup(pages::not_found()));
                };

                let Some(component) = entity.dyn_component(component) else {
                    return Ok(Response::NotFound);
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
                    return Ok(Response::NotFound);
                };

                entity.attach(LastAccess::now());

                Ok(Response::Download {
                    filename,
                    content_type,
                    data,
                })
            }
        }
    }
}

impl std::fmt::Debug for Response {
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

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index => f.debug_tuple("Index").finish(),
            Self::Entities { filter } => f.debug_tuple("Entities").field(&filter).finish(),
            Self::Entity(eid) => f.debug_tuple("Entity").field(eid).finish(),
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
            Self::DownloadComponent {
                entity_id: entity,
                component,
            } => f
                .debug_tuple("Download")
                .field(entity)
                .field(component)
                .finish(),
        }
    }
}

fn deserialize_comma_separated_string<'de, V, T, D>(deserializer: D) -> Result<V, D::Error>
where
    V: FromIterator<T>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
    D: serde::de::Deserializer<'de>,
{
    struct CommaSeparated<V, T>(PhantomData<V>, PhantomData<T>);

    impl<V, T> serde::de::Visitor<'_> for CommaSeparated<V, T>
    where
        V: FromIterator<T>,
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        type Value = V;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("string containing comma-separated elements")
        }

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let iter = s
                .split(",")
                .filter(|s| !s.is_empty())
                .map(std::str::FromStr::from_str);
            Result::from_iter(iter).map_err(serde::de::Error::custom)
        }
    }

    let visitor = CommaSeparated(PhantomData, PhantomData);
    deserializer.deserialize_str(visitor)
}

fn serialize_comma_separated_string<S, V>(value: V, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    V: IntoIterator,
    V::Item: ToString + Debug,
{
    let s = value
        .into_iter()
        .map(|v| form_urlencoded::byte_serialize(v.to_string().as_bytes()).collect::<String>())
        .collect::<Vec<_>>()
        .join(",");
    s.serialize(ser)
}
