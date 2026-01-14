use axum::http::StatusCode;
use ecsdb::{Component, EntityId};
use serde::{Deserialize, Serialize};
use tower::Service as _;
use tower_http::ServiceExt as _;
use tracing::debug;

use std::task::{Context, Poll};

use futures_util::future::BoxFuture;
use http::Method;

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

pub fn service<RequestBody, DbFun>(
    open_db: DbFun,
) -> impl tower::Service<
    http::Request<RequestBody>,
    Response = http::Response<ResponseBody>,
    Error = Error,
    Future = impl Future<Output = Result<http::Response<ResponseBody>, Error>>,
> + Clone
where
    RequestBody: http_body::Body + Send + 'static,
    RequestBody::Data: Send,
    DbFun:
        Fn(&http::Request<RequestBody>) -> Result<ecsdb::Ecs, ecsdb::Error> + Send + Copy + 'static,
{
    tower::ServiceBuilder::new().service_fn(move |request: http::Request<RequestBody>| {
        let is_hx_request = request
            .headers()
            .get("HX-Request")
            .is_some_and(|h| h.to_str().is_ok_and(|v| v == "true"));
        let is_hx_boosted = request
            .headers()
            .get("HX-Boosted")
            .is_some_and(|h| h.to_str().is_ok_and(|v| v == "true"));

        let is_htmx_request = is_hx_request && !is_hx_boosted;

        let service = Service { open_db };
        service
            .map_response_body(move |markup| {
                let markup = if is_htmx_request {
                    markup
                } else {
                    pages::wrap_in_body(markup)
                };

                ResponseBody::from(markup.into_string())
            })
            .call(request)
    })
}

#[derive(Clone, Default)]
struct Service<DbFun> {
    open_db: DbFun,
}

type ResponseBody = String;
// type Response = http::Response<ResponseBody>;
type Error = String;

impl<RequestBody, DbFun> tower::Service<http::Request<RequestBody>> for Service<DbFun>
where
    DbFun:
        Fn(&http::Request<RequestBody>) -> Result<ecsdb::Ecs, ecsdb::Error> + Send + Copy + 'static,
    RequestBody: http_body::Body + Send + 'static,
    RequestBody::Data: Send,
{
    type Response = http::Response<maud::Markup>;
    type Error = Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<RequestBody>) -> Self::Future {
        let open_db = self.open_db;

        Box::pin(async move {
            debug!(method = ?req.method(), path = ?req.uri().path());

            let db = tokio::task::block_in_place(|| open_db(&req)).map_err(|e| e.to_string())?;

            let url = url::Url::parse("http://localhost")
                .unwrap()
                .join(&req.uri().to_string())
                .map_err(|e| e.to_string())?;
            let path_components: Box<[&str]> =
                url.path_segments().map(|s| s.collect()).unwrap_or_default();

            match (req.method(), path_components.iter().as_slice()) {
                (_, components) if components.last().is_some_and(|last| last.is_empty()) => {
                    Ok(http::Response::builder()
                        .status(http::StatusCode::SEE_OTHER)
                        .header(http::header::LOCATION, url.path().trim_end_matches('/'))
                        .body(maud::html!({}))
                        .unwrap())
                }

                (&Method::GET, &["entities", entity_id]) => {
                    let Ok(entity_id) = str::parse::<EntityId>(entity_id) else {
                        return Err(format!("Invalid eid {entity_id}"));
                    };

                    let entity = db.entity(entity_id);
                    entity.attach(LastAccess::now());
                    Ok(html_response(pages::entity(entity)))
                }

                _ => Ok(http::Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(maud::html!({}))
                    .unwrap()),
            }
        })
    }
}

fn html_response(markup: maud::Markup) -> http::Response<maud::Markup> {
    http::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html")
        .body(markup)
        .unwrap()
}

mod pages {
    use maud::{Markup, html};
    pub fn entity(entity: ecsdb::Entity) -> Markup {
        html!({
            table {
                tr {
                    td { "eid" }
                    td {
                        (entity.id())
                    }
                }
                tr {
                    td { "Components" }
                    td {
                        @for name in entity.component_names() {
                            (name)
                        }
                    }
                }
            }
        })
    }

    pub fn wrap_in_body(contents: Markup) -> Markup {
        html! {
            html {
                body {
                    (contents)
                }
            }
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
