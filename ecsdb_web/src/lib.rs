use std::{borrow::Cow, collections::HashMap, convert::Infallible};

use bytes::Bytes;
use ecsdb::{Component, EntityId};
use http::{StatusCode, header};

use http_body_util::BodyExt as _;
use iref::iri;
use serde::{Deserialize, Serialize};
use tower::{ServiceBuilder, service_fn};

use futures_util::{FutureExt, TryFutureExt, TryStreamExt};
use http::Method;
use tower_http::ServiceExt;
use tracing::{debug, instrument};

#[derive(Serialize, Deserialize, Component, Debug)]
pub struct LastAccess(pub chrono::DateTime<chrono::Utc>);

impl LastAccess {
    pub fn now() -> Self {
        Self(chrono::Utc::now())
    }
}

// mod list;
// use list::list;

// mod htmx;
// use htmx::HtmxTemplate;

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

pub fn service<RequestBody, DbFun>(
    base_path: &str,
    open_db: DbFun,
) -> impl tower::Service<
    http::Request<RequestBody>,
    Response = http::Response<ResponseBody>,
    Error = Infallible,
    Future = impl Future<Output = Result<http::Response<ResponseBody>, Infallible>>,
> + Clone
where
    RequestBody: http_body::Body<Data = Bytes> + Send + 'static + Unpin,
    DbFun: Fn(&http::Request<RequestBody>) -> Result<ecsdb::Ecs, ecsdb::Error>
        + Send
        + Sync
        + Copy
        + 'static,
    <RequestBody as http_body::Body>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let base_uri = http::Uri::try_from(
        {
            if base_path.ends_with('/') {
                Cow::Borrowed(base_path)
            } else {
                Cow::Owned(format!("{base_path}/"))
            }
        }
        .as_bytes(),
    )
    .unwrap();

    async fn ecs_service<RB, DbFun>(
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

        match kind.handle(db).await? {
            EcsResponse::Markup(markup) => {
                let markup = if is_htmx_request {
                    markup
                } else {
                    let breadcrumbs = kind.breadcrumbs();
                    pages::wrap_in_body(&base_url, &breadcrumbs, markup)
                };

                http::Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html")
                    .body(ResponseBody::from(markup.into_string()))
                    .map_err(|e| Error::Other(Box::new(e)))
            }
            EcsResponse::Redirect(path) => {
                // Prepent base url to our redirect target
                let mut base = iri::PathBuf::new(base_url.path().to_owned()).unwrap();
                base.symbolic_append(path.segments());

                http::Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::LOCATION, base.as_str())
                    .body(ResponseBody::new(Bytes::new()))
                    .map_err(|e| Error::Other(Box::new(e)))
            }
        }
    }

    macro_rules! include_assets {
        ([ $( ($asset:literal, $content_type:literal) ),* ] ) => {{
            let mut assets = HashMap::new();
                $(
                    assets.insert($asset, {
                        static ASSET: &[u8] = include_bytes!($asset);
                        http::Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", $content_type)
                            .body(ResponseBody::from(ASSET))
                            .unwrap()
                    });
                )*
                assets

        }};
    }

    let assets = include_assets!([
        ("missing.css", "text/css"),
        ("htmx.js", "application/javascript")
    ]);

    let service = service_fn(move |req: http::Request<RequestBody>| {
        let response = if req.method() == Method::GET
            && let Some(last_path_element) = req.uri().path().rsplit('/').next()
            && let Some(asset) = assets.get(last_path_element)
        {
            Box::pin(futures_util::future::ready(asset.clone()))
        } else {
            ecs_service(base_uri.clone(), open_db, req)
                .unwrap_or_else(|e| e.into_response())
                .boxed()
        };

        response.map(Ok)
    });

    ServiceBuilder::new().service(service).trim_trailing_slash()
}

type ResponseBody = http_body_util::Full<bytes::Bytes>;

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
}

struct Breadcrumb {
    title: String,
    path: Option<iri::PathBuf>,
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

    fn breadcrumbs(&self) -> Vec<Breadcrumb> {
        let mut breadcrumbs = vec![Breadcrumb {
            title: "Entities".into(),
            path: Some(iri::PathBuf::new("entities".into()).expect("Valid iri::PathBuf")),
        }];

        fn add(breadcrumbs: &mut Vec<Breadcrumb>, title: &str, subpath: &[&str]) {
            let mut path = breadcrumbs
                .iter()
                .rfind(|b| b.path.is_some())
                .and_then(|b| b.path.to_owned())
                .unwrap(); // SAFETY: There is at least one entry with a path (the root)

            path.normalize();

            for element in subpath {
                path.symbolic_push(iri::Segment::new(element).expect("Valid iri::Segment"));
            }
            breadcrumbs.push(Breadcrumb {
                title: title.into(),
                path: Some(path),
            })
        }

        match self {
            RequestType::Entities => (),
            RequestType::Entity(eid) => {
                let eid = eid.to_string();
                add(&mut breadcrumbs, &eid, &[&eid]);
            }
            RequestType::Component {
                entity_id,
                component,
            } => {
                let entity_id = entity_id.to_string();
                add(&mut breadcrumbs, &entity_id, &[&entity_id]);
                add(&mut breadcrumbs, component, &["components", component]);
            }
            RequestType::ModifyComponent { .. } => unreachable!(),
        };

        breadcrumbs
    }
}

enum EcsResponse {
    Markup(maud::Markup),
    Redirect(iri::PathBuf),
}

impl RequestType {
    #[instrument(level = "debug", skip(db), ret)]
    async fn handle(&self, db: ecsdb::Ecs) -> Result<EcsResponse, Error> {
        match *self {
            RequestType::Entities => {
                let entities = db.query::<ecsdb::Entity, ()>();
                Ok(EcsResponse::Markup(pages::entities(entities)))
            }
            RequestType::Entity(eid) => {
                let Some(entity) = db.find(eid).next() else {
                    return Ok(EcsResponse::Markup(pages::not_found()));
                };

                entity.attach(LastAccess::now());
                Ok(EcsResponse::Markup(pages::entity(entity)))
            }
            RequestType::Component {
                entity_id: entity,
                ref component,
            } => {
                let Some(entity) = db.find(entity).next() else {
                    return Ok(EcsResponse::Markup(pages::not_found()));
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
                    return Ok(EcsResponse::Markup(pages::not_found()));
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
        }
    }
}

mod pages {
    use maud::{Markup, html};

    use crate::Breadcrumb;
    pub fn entity(entity: ecsdb::Entity) -> Markup {
        html!({
            table {
                    @for name in entity.component_names() {
                        @let component = entity.dyn_component(&name).unwrap();
                        tr {
                            td {
                                a href=(format!("entities/{}/components/{}", entity.id(), name)) {
                                    pre { (name) }
                                }
                            }
                            td {
                                pre {
                                    (component.as_json().map(|j| j.to_string()).unwrap_or_else(|| "<unrenderable>".to_string()))
                                }
                            }
                        }
                    }
            }
        })
    }

    pub fn entities<'a>(entities: impl IntoIterator<Item = ecsdb::Entity<'a>>) -> Markup {
        html!({
            table {
                tr {
                    th { "EntityId" }
                    th { "Components" }
                }
                @for entity in entities.into_iter() {
                    tr {
                        td {
                            a href=(format!("entities/{}", entity.id())) {
                                pre { (entity.id()) }
                            }
                        }
                        td {
                            pre {
                                @for name in entity.component_names() {
                                    (name) ", "
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    pub fn component_editor(entity: ecsdb::Entity, component_name: &str) -> Markup {
        let Some(component) = entity.dyn_component(component_name) else {
            return not_found();
        };

        let Some(component_json) = component.as_json() else {
            return not_found();
        };

        let json = serde_json::to_string_pretty(&component_json).expect("component -> json");

        html! {
            h2 { (format!("Editing {component_name} of {entity}")) }
            form method="post" {
                pre {
                    textarea name="component_data" class="width:100%" rows=(json.lines().count()) {
                        (json)
                    }
                }
                input type="submit" {}
            }
        }
    }

    pub fn not_found() -> Markup {
        html!({ "not found" })
    }

    pub fn wrap_in_body(
        base_url: &http::Uri,
        breadcrumbs: &[Breadcrumb],
        contents: Markup,
    ) -> Markup {
        html! {
            html {
                head {
                    link rel="stylesheet" href="missing.css" {}
                    script src="htmx.js" r#type="application/javascript" {}
                    base href=(base_url) { }
                }
                body {
                    header {
                        nav.breadcrumbs aria-label="Breadcrumbs" {
                            ul role="list" {
                                @for (n, breadcrumb) in (breadcrumbs.iter().enumerate()) {
                                    @let is_last = n == breadcrumbs.len()-1;
                                    li.inline aria-current=[is_last.then_some("page")] {
                                        a href=[breadcrumb.path.clone()] { (breadcrumb.title) }
                                    }
                                }
                            }
                        }
                    }
                    main {
                        (contents)
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for EcsResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Markup(_markup) => f.debug_tuple("Markup").field(&"<html>").finish(),
            Self::Redirect(path) => f.debug_tuple("Redirect").field(path).finish(),
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
        }
    }
}

// struct GenericListTemplate;
// impl list::Template for GenericListTemplate {
//     fn list_template() -> &'static str {
//         "entities/list.html"
//     }

//     fn detail_template() -> &'static str {
//         "entities/show.html"
//     }
// }

//  async fn entity<T: list::Template>(
//     db: ExtractDatabase,
//     entity: axum::extract::Path<EntityId>,
// ) -> impl IntoResponse {
//     let db = db.acquire().await;

//     let Some(entity) = db.entity(entity.0).or_none() else {
//         return (StatusCode::NOT_FOUND, "").into_response();
//     };

//     let context = entity_context(entity);

//     HtmxTemplate {
//         template_name: T::detail_template(),
//         context: json!({
//             "entity": context,
//         }),
//     }
//     .into_response()
// }

//  mod editable {
//     use std::{convert::Infallible, fmt::Debug, marker::PhantomData};

//     use anyhow::anyhow;
//     use axum::{
//         Router,
//         extract::{self, FromRequest},
//         response::IntoResponse,
//         routing::get,
//     };
//     use ecsdb::{Component, EntityId};
//     use http::{StatusCode, header};
//     use serde::Deserialize;
//     use serde_json::json;
//     use tracing::{error, info};

//     use crate::{AppState, api::HtmxTemplate};

//      type EditEntityError = anyhow::Error;
//      type FormValue = String;

//      trait Kind {
//         const HTML_INPUT_TYPE: &'static str;

//         type Inner;
//         type FromInnerError;
//         fn from_form_value(form_value: FormValue) -> Result<Self::Inner, Self::FromInnerError>;
//         fn to_form_value(value: Self::Inner) -> FormValue;
//     }

//     #[derive(Debug, PartialEq, Eq, Clone, Copy)]
//      struct TextField;
//     impl Kind for TextField {
//         const HTML_INPUT_TYPE: &'static str = "text";

//         type Inner = String;
//         type FromInnerError = Infallible;

//         fn from_form_value(form_value: FormValue) -> Result<Self::Inner, Self::FromInnerError> {
//             Ok(form_value)
//         }

//         fn to_form_value(value: Self::Inner) -> FormValue {
//             value
//         }
//     }

//     #[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
//      struct NumberField<N: Debug>(PhantomData<N>);
//     impl<N: std::str::FromStr + ToString + Clone + Copy + Debug> Kind for NumberField<N> {
//         const HTML_INPUT_TYPE: &'static str = "number";

//         type Inner = N;
//         type FromInnerError = <N as std::str::FromStr>::Err;
//         fn from_form_value(form_value: FormValue) -> Result<Self::Inner, Self::FromInnerError> {
//             N::from_str(&form_value)
//         }

//         fn to_form_value(value: Self::Inner) -> FormValue {
//             value.to_string()
//         }
//     }

//     #[derive(Debug, PartialEq, Eq, Clone, Copy)]
//      struct UriField;
//     impl Kind for UriField {
//         const HTML_INPUT_TYPE: &'static str = "url";

//         type Inner = url::Url;
//         type FromInnerError = url::ParseError;
//         fn from_form_value(form_value: FormValue) -> Result<Self::Inner, Self::FromInnerError> {
//             url::Url::parse(&form_value)
//         }

//         fn to_form_value(value: Self::Inner) -> FormValue {
//             value.to_string()
//         }
//     }

//      trait EditableComponent: Debug + Send + Sync {
//         fn component_name(&self) -> &'static str;
//         fn retrieve(&self, entity: ecsdb::Entity) -> Result<Option<FormValue>, EditEntityError>;
//         fn persist(
//             &self,
//             entity: ecsdb::Entity,
//             form_data: FormValue,
//         ) -> Result<(), EditEntityError>;

//         fn input_attributes(&self) -> &[(&'static str, &'static str)];
//     }

//     //  trait Editable {
//     //     fn to_form(self) -> String;
//     //     fn try_from_form(value: &str) -> Result<Self, anyhow::Error>;
//     // }

//     // macro_rules! editable {
//     //     ( $($ts:ty),* ) => {
//     //         [$(make_editable::<$ts>(),)*];
//     //     }
//     // }

//      fn make_editable<
//         C: Component<Storage = ecsdb::component::JsonStorage>
//             + TryFrom<K::Inner>
//             + Into<K::Inner>
//             + Debug
//             + Send
//             + Sync,
//         K: Kind + Debug + Send + Sync + 'static,
//     >() -> Box<dyn EditableComponent> {
//         #[derive(Debug)]
//          struct GenericEditableComponent<C, K>(PhantomData<C>, PhantomData<K>);

//         impl<
//             T: Component<Storage = ecsdb::component::JsonStorage>
//                 + TryFrom<K::Inner>
//                 + Into<K::Inner>
//                 + Debug
//                 + Send
//                 + Sync,
//             K: Kind + Debug + Send + Sync + 'static,
//         > EditableComponent for GenericEditableComponent<T, K>
//         {
//             fn component_name(&self) -> &'static str {
//                 T::NAME
//             }

//             fn retrieve(
//                 &self,
//                 entity: ecsdb::Entity,
//             ) -> Result<Option<FormValue>, EditEntityError> {
//                 let Some(component) = entity.try_component::<T>()? else {
//                     return Ok(None);
//                 };

//                 let serialized: K::Inner = component.into();
//                 let form_value = K::to_form_value(serialized);
//                 Ok(Some(form_value))
//             }

//             fn persist(
//                 &self,
//                 entity: ecsdb::Entity,
//                 form_data: FormValue,
//             ) -> Result<(), EditEntityError> {
//                 // let c: T = T::Storage::from_json(value).unwrap();

//                 let deserialized: K::Inner = K::from_form_value(form_data)
//                     .map_err(|_| anyhow!("Failed to deserialize value"))?;
//                 let component: T = T::try_from(deserialized)
//                     .map_err(|_| anyhow!("Failed to deserialize value"))?;

//                 entity.try_attach(component)?;

//                 Ok(())
//             }

//             fn input_attributes(&self) -> &[(&'static str, &'static str)] {
//                 &[("type", K::HTML_INPUT_TYPE)]
//             }
//         }

//         Box::new(GenericEditableComponent(PhantomData::<C>, PhantomData::<K>))
//     }

//     (crate) fn routes() -> axum::Router<AppState<'static>> {
//         async fn get_component(
//             state: extract::State<AppState<'static>>,
//             db: super::ExtractDatabase,
//             request_headers: reqwest::header::HeaderMap,
//             extract::Path((entity, component)): extract::Path<(EntityId, String)>,
//         ) -> impl IntoResponse {
//             let Some(current_url) = request_headers
//                 .get("Hx-Current-URL")
//                 .and_then(|h| h.to_str().ok())
//                 .map(|h| h.to_string())
//             else {
//                 error!(?request_headers, "Missing 'HX-Current-URL'");
//                 return (StatusCode::BAD_REQUEST, "").into_response();
//             };

//             let db = db.acquire().await;

//             let Some(editable_component) = state.editable_components.get(component.as_str()) else {
//                 return (
//                     StatusCode::NOT_FOUND,
//                     format!("Couldn't find Entity {entity}"),
//                 )
//                     .into_response();
//             };

//             let Some(entity) = db.entity(entity).or_none() else {
//                 return (
//                     StatusCode::NOT_FOUND,
//                     format!("Couldn't find Entity {entity}"),
//                 )
//                     .into_response();
//             };

//             let component_json = match editable_component.retrieve(entity) {
//                 Ok(json) => json,
//                 Err(e) => {
//                     return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
//                 }
//             };

//             HtmxTemplate {
//                 template_name: "components/edit.html",
//                 context: json!({
//                     "id": entity.id(),
//                     "component": component,
//                     "data": component_json,
//                     "input_attributes": editable_component.input_attributes(),
//                     "links": {
//                         "submit": format!("/entities/{}/components/{}", entity.id(), component),
//                         "reset": current_url
//                     }
//                 }),
//             }
//             .into_response()
//         }

//         async fn post_component(
//             state: extract::State<AppState<'static>>,
//             db: super::ExtractDatabase,
//             request_headers: reqwest::header::HeaderMap,
//             extract::Path((entity, component)): extract::Path<(EntityId, String)>,
//             request: extract::Request,
//         ) -> impl IntoResponse {
//             let Some(current_url) = request_headers
//                 .get("Hx-Current-URL")
//                 .and_then(|h| h.to_str().ok())
//                 .map(|h| h.to_string())
//             else {
//                 error!(?request_headers, "Missing 'HX-Current-URL'");
//                 return (StatusCode::BAD_REQUEST, "").into_response();
//             };

//             let Some(editable_component) = state.editable_components.get(component.as_str()) else {
//                 return (
//                     StatusCode::BAD_REQUEST,
//                     format!("Component {component} not editable"),
//                 )
//                     .into_response();
//             };

//             #[derive(Debug, Deserialize)]
//             struct ComponentForm {
//                  component: String,
//             }

//             match extract::Form::<ComponentForm>::from_request(request, &()).await {
//                 Ok(data) => {
//                     let db = db.acquire().await;
//                     let Some(entity) = db.entity(entity).or_none() else {
//                         return (
//                             StatusCode::NOT_FOUND,
//                             format!("Couldn't find Entity {entity}"),
//                         )
//                             .into_response();
//                     };

//                     info!(
//                         component.name = component,
//                         component.data = ?data.0.component,
//                         "Updating"
//                     );

//                     match editable_component.persist(entity, data.0.component) {
//                         Ok(()) => (StatusCode::SEE_OTHER, [(header::LOCATION, current_url)], "")
//                             .into_response(),
//                         Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
//                     }
//                 }
//                 Err(e) => {
//                     error!(?e);
//                     (StatusCode::BAD_REQUEST, e.to_string()).into_response()
//                 }
//             }
//         }

//         // async fn put_component() -> impl IntoResponse {
//         //     todo!()
//         // }

//         // async fn delete_component() -> impl IntoResponse {
//         //     todo!()
//         // }

//         Router::new().route("/", get(get_component).post(post_component))
//     }
// }

// fn list_link(entity_type: VelodbEntity) -> String {
//     let path = match entity_type {
//         VelodbEntity::Unknown => "entities",
//         VelodbEntity::Session => "sessions",
//         VelodbEntity::Route => "routes",
//         VelodbEntity::Annotation => "annotations",
//     };

//     format!("/{path}")
// }

//  fn self_link(entity_type: VelodbEntity, id: EntityId) -> String {
//     let path = match entity_type {
//         VelodbEntity::Unknown => "entities",
//         VelodbEntity::Session => "sessions",
//         VelodbEntity::Route => "routes",
//         VelodbEntity::Annotation => "annotations",
//     };

//     format!("/{path}/{id}")
// }

//  async fn index(state: extract::State<AppState<'_>>) -> impl IntoResponse {
//     Html(state.render_template("index.html", context! {}))
// }

// #[derive(rust_embed::Embed, Debug, Copy, Clone)]
// #[folder = "src/templates/static/"]
//  struct Assets;

// fn deserialize_comma_separated_string<'de, V, T, D>(deserializer: D) -> Result<V, D::Error>
// where
//     V: FromIterator<T>,
//     T: FromStr,
//     T::Err: Display,
//     D: serde::de::Deserializer<'de>,
// {
//     struct CommaSeparated<V, T>(PhantomData<V>, PhantomData<T>);

//     impl<V, T> serde::de::Visitor<'_> for CommaSeparated<V, T>
//     where
//         V: FromIterator<T>,
//         T: FromStr,
//         T::Err: Display,
//     {
//         type Value = V;

//         fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
//             formatter.write_str("string containing comma-separated elements")
//         }

//         fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
//         where
//             E: serde::de::Error,
//         {
//             let iter = s
//                 .split(",")
//                 .filter(|s| !s.is_empty())
//                 .map(FromStr::from_str);
//             Result::from_iter(iter).map_err(serde::de::Error::custom)
//         }
//     }

//     let visitor = CommaSeparated(PhantomData, PhantomData);
//     deserializer.deserialize_str(visitor)
// }

// use axum::extract::FromRequestParts;

// impl axum::extract::FromRequestParts<Self> for &AppState<'_> {
//     type Rejection = <axum::extract::State<Self> as FromRequestParts<Self>>::Rejection;

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         _req: &mut http::request::Parts,
//         state: &Self,
//     ) -> Result<Self, Self::Rejection> {
//         // let app_state = axum::extract::State::<Self>::from_request_parts(req, state).await?;
//         // Ok(app_state)
//         Ok(state)
//     }
// }

// impl axum::extract::FromRequestParts<AppState<'_>> for UserSession {
//     type Rejection = (http::StatusCode, &'static str);

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         req: &mut http::request::Parts,
//         state: &AppState<'_>,
//     ) -> Result<Self, Self::Rejection> {
//         Ok(Option::<Self>::from_request_parts(req, state)
//             .await?
//             .expect("UserSession"))
//     }
// }

// impl axum::extract::OptionalFromRequestParts<AppState<'_>> for UserSession {
//     type Rejection = (http::StatusCode, &'static str);

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         req: &mut http::request::Parts,
//         state: &AppState<'_>,
//     ) -> Result<Option<Self>, Self::Rejection> {
//         Ok(
//             <extract::Extension<UserSession> as extract::FromRequestParts<_>>::from_request_parts(
//                 req, state,
//             )
//             .await
//             .map(|ext| ext.0)
//             .ok(),
//         )
//     }
// }

// impl axum::extract::FromRequestParts<AppState<'_>> for wahoo::AccessToken {
//     type Rejection = (http::StatusCode, &'static str);

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         req: &mut http::request::Parts,
//         state: &AppState<'_>,
//     ) -> Result<Self, Self::Rejection> {
//         Ok(
//             <Self as extract::OptionalFromRequestParts<_>>::from_request_parts(req, state)
//                 .await?
//                 .expect("AccessToken component"),
//         )
//     }
// }

// impl axum::extract::OptionalFromRequestParts<AppState<'_>> for wahoo::AccessToken {
//     type Rejection = (http::StatusCode, &'static str);

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         req: &mut http::request::Parts,
//         state: &AppState<'_>,
//     ) -> Result<Option<Self>, Self::Rejection> {
//         let Some(session) = Option::<UserSession>::from_request_parts(req, state).await? else {
//             return Ok(None);
//         };

//         let Some(db) = Option::<ExtractDatabase>::from_request_parts(req, state).await? else {
//             return Ok(None);
//         };

//         let db = db.acquire().await;
//         let Some(account_entity) = db.find(wahoo::UserId(session.user_id().to_string())).next()
//         else {
//             return Ok(None);
//         };

//         Ok(account_entity.component::<Self>())
//     }
// }

// #[derive(Debug, Clone)]
//  struct ExtractDatabase(PathBuf);

// impl ExtractDatabase {
//      fn open(&self) -> ecsdb::Ecs {
//         ecsdb::Ecs::open(self.path()).unwrap()
//     }

//      async fn acquire(&self) -> ecsdb::Ecs {
//         tokio::task::block_in_place(|| self.open())
//     }

//      fn path(&self) -> &std::path::Path {
//         self.0.as_path()
//     }
// }

// impl axum::extract::FromRequestParts<AppState<'_>> for ExtractDatabase {
//     type Rejection = (http::StatusCode, &'static str);

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         req: &mut http::request::Parts,
//         state: &AppState<'_>,
//     ) -> Result<Self, Self::Rejection> {
//         if let Some(state) = Option::<Self>::from_request_parts(req, state).await? {
//             Ok(state)
//         } else {
//             Err((
//                 http::StatusCode::INTERNAL_SERVER_ERROR,
//                 "Couldn't extract AppState",
//             ))
//         }
//     }
// }

// impl axum::extract::OptionalFromRequestParts<AppState<'_>> for ExtractDatabase {
//     type Rejection = (http::StatusCode, &'static str);

//     #[tracing::instrument(level = "trace", skip_all)]
//     async fn from_request_parts(
//         req: &mut http::request::Parts,
//         state: &AppState<'_>,
//     ) -> Result<Option<Self>, Self::Rejection> {
//         let Some(user_session) = Option::<UserSession>::from_request_parts(req, state).await?
//         else {
//             return Ok(None);
//         };

//         // let database =
//         //     tokio::task::block_in_place(|| ecsdb::Ecs::open(&user_session.database_path).unwrap());

//         Ok(Some(ExtractDatabase(
//             user_session.database_path(&state.config),
//         )))
//     }
// }
