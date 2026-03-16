use std::{borrow::Cow, collections::HashMap, convert::Infallible};

use bytes::Bytes;
use ecsdb::Component;

use iref::iri;
use serde::{Deserialize, Serialize};
use tower::{ServiceBuilder, service_fn};

use futures_util::{FutureExt, TryFutureExt};
use http::Method;
use tower_http::ServiceExt;

use crate::ecs_service::Request;

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

mod ecs_service;
pub use ecs_service::ecs_service;

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
        + Clone
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

    macro_rules! include_assets {
        ([ $( ($asset:literal, $content_type:literal) ),* ] ) => {{
            let mut assets = HashMap::new();
                $(
                    assets.insert($asset, {
                        static ASSET: &[u8] = include_bytes!($asset);
                        http::Response::builder()
                            .status(http::StatusCode::OK)
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
            let open_db = open_db.clone();
            ecs_service(base_uri.clone(), open_db, req)
                .unwrap_or_else(|e| e.into_response())
                .boxed()
        };

        response.map(Ok)
    });

    ServiceBuilder::new().service(service).trim_trailing_slash()
}

type ResponseBody = http_body_util::Full<bytes::Bytes>;

struct Breadcrumb {
    title: String,
    path: Option<iri::PathBuf>,
}

impl Breadcrumb {
    fn from_request(request: &Request) -> Vec<Breadcrumb> {
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

        match request {
            Request::Index => (),
            Request::Entities { .. } => (),
            Request::Entity(eid) => {
                let eid = eid.to_string();
                add(&mut breadcrumbs, &eid, &[&eid]);
            }
            Request::Component {
                entity_id,
                component,
            } => {
                let entity_id = entity_id.to_string();
                add(&mut breadcrumbs, &entity_id, &[&entity_id]);
                add(&mut breadcrumbs, component, &["components", component]);
            }
            Request::ModifyComponent { .. } => unreachable!(),
            Request::DeleteComponent { .. } => unreachable!(),
            Request::DownloadComponent { .. } => unreachable!(),
        };

        breadcrumbs
    }
}

mod pages {
    use ecsdb::DynComponent;
    use maud::{Markup, html};
    use std::fmt::Display;

    use crate::Breadcrumb;
    pub fn entity(entity: ecsdb::Entity) -> Markup {
        html!({
            table {
                thead {
                    tr {
                        th { "Component" }
                        th { "Type" }
                        th { "Data" }
                        th {}
                    }
                }
                tbody {
                    @for name in entity.component_names() {
                        @let component = entity.dyn_component(&name).unwrap();
                        @let url = format!("entities/{}/components/{}", entity.id(), name);
                        tr {
                            td {
                                a href=(url) {
                                    pre { (name) }
                                }
                            }
                            td {
                                pre {
                                    (format!("{:?}", component.kind()))
                                }
                            }
                            td {
                                pre {
                                    (component.as_json().map(|j| j.to_string()).unwrap_or_else(|| "<unrenderable>".to_string()))
                                }
                            }
                            td {
                                form {
                                    @let confirm = format!("Delete '{name}' from entity {}?", entity.id());
                                    button hx-delete=(url) hx-confirm=(confirm) hx-target="closest table" {
                                        "␡"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    fn format_time<Tz>(datetime: chrono::DateTime<Tz>) -> maud::Markup
    where
        Tz: chrono::TimeZone + Copy,
        Tz::Offset: Display + Copy,
    {
        let now = chrono::Utc::now().with_timezone(&datetime.timezone());
        let duration = now.signed_duration_since(datetime);
        let full_days = (now.date_naive() - datetime.date_naive()).num_days();

        let human = match (full_days, duration.num_hours(), duration.num_minutes()) {
            (0, 0, 0) => "just now".to_string(),
            (0, 0, 1) => "1 minute ago".to_string(),
            (0, 0, m) => format!("{m} minutes ago"),
            (0, 1, _) => "1 hour ago".to_string(),
            (0, h, _) => format!("{h} hours ago"),
            (1, h @ 0..=23, _) => format!("{h} hours ago"),
            (1, _, _) => "yesterday".to_string(),
            (d @ ..=6, _, _) => format!("{d} days ago"),
            (7, _, _) => "1 week ago".to_string(),
            _ if datetime == chrono::DateTime::<Tz>::MIN_UTC => "-".into(),
            _ => datetime.format("%Y-%m-%d").to_string(),
        };

        html! {
            time datetime=(datetime.to_rfc3339()) { (human) }
        }
    }

    pub fn entities<'a>(
        entities: &[ecsdb::Entity<'a>],
        filter: &crate::ecs_service::Filter,
        all_component_names: &[impl AsRef<str>],
    ) -> Markup {
        let mut all_component_names = all_component_names
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>();
        all_component_names.sort();

        html!({
            form action="entities" hx-get="entities" hx-trigger="change,submit" hx-target="this" hx-swap="outerHTML" hx-push-url="true" {

                div class="flex-row" {
                    div {
                        label for="component_input" { "Component" }
                        select id="component_input" name="component_names" {
                            option value="" selected[filter.component_names.is_empty()] { "-" }
                            @for component_name in all_component_names {
                                @let selected = filter.component_names.contains(component_name);
                                option value=(component_name) selected[selected] { (component_name) }
                            }
                        }
                    }

                    div{
                        label for="count_input" { "Show" }
                        select id="count_input" name="count" {
                            @for n in [20, 50, 100] {
                                @let selected = filter.count == n;
                                option value=(n) selected[selected] { (n) }
                            }
                            @let selected = filter.count == 999999;
                            option value="999999" selected[selected] { "All" }
                        }
                    }

                    div class="align-self:end" {
                        button type="submit" name="after" value="0" { "Apply "}
                    }
                }

                style {r#"
                    #entity-table th, #entity-table td {
                        text-wrap: nowrap;
                    }
                "#}

                table id="entity-table" {
                    thead {
                        tr {
                            th { "EntityId" }
                            @for component_name in &filter.component_names {
                                th { (component_name) }
                            }
                            th { "Created" }
                            th { "Updated" }
                            th { "Components" }
                            th { "View" }
                        }
                    }
                    tbody {
                        @for entity in entities {
                            @let popover_id = format!("entity-{}", entity.id());
                            tr {
                                td {
                                    a href=(format!("entities/{}", entity.id())) {
                                        pre { (entity.id()) }
                                    }
                                }
                                @for component_name in &filter.component_names {
                                    td style="overflow-x: scroll; max-width: 300px;" {
                                        @let component = entity.dyn_component(component_name);
                                        (component_inline_view(component))
                                    }
                                }
                                td {
                                    (format_time(entity.created_at()))
                                }
                                td {
                                    (format_time(entity.last_modified()))
                                }
                                td title=(entity.component_names().collect::<Vec<_>>().join(", ")) {
                                    (entity.component_names().count()) " Components"
                                }
                                td style="text-align: center" {
                                    a class="<button>" hx-on:click=(format!("htmx.find('#{popover_id}').togglePopover()")) {
                                        "View"
                                    }
                                    dialog id=(popover_id) popover="auto" {
                                        div class="titlebar flex-row justify-content:space-between align-items:center" {
                                            span { "Entity " (entity.id()) }
                                            button type="button" popovertarget=(popover_id) popovertargetaction="hide" {
                                                "Close"
                                            }
                                        }
                                        div hx-get=(format!("entities/{}", entity.id()))
                                            hx-trigger=(format!("toggle from:#{popover_id}"))
                                            hx-swap="innerHTML" {
                                            "Loading..."
                                        }
                                    }
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

        let content_editor = match component.kind() {
            ecsdb::dyn_component::Kind::Json => {
                let Some(component_json) = component.as_json() else {
                    return not_found();
                };

                let json =
                    serde_json::to_string_pretty(&component_json).expect("component -> json");

                html! {
                    pre {
                        textarea name="component_data" class="width:100%" rows=(json.lines().count()) {
                            (json)
                        }
                    }
                    input type="submit" {}
                }
            }
            ecsdb::dyn_component::Kind::Blob => {
                let Some(blob) = component.as_blob() else {
                    return not_found();
                };

                html! {
                    a href=(format!("entities/{}/components/{}/download", entity.id(), component_name)) {
                        "Download (" (blob.len()) " bytes)"
                    }

                    details {
                        summary { "Base64" }
                        textarea style="width:100%" rows="100" readonly hx-on:click="this.select()" {
                            (data_encoding::BASE64.encode(blob))
                        }
                    }
                }
            }
            ecsdb::dyn_component::Kind::Null => {
                html! {
                    pre { "null" }
                }
            }
            ecsdb::dyn_component::Kind::Other(t) => html! {
                p { "Unsupported data type " (format!("'{t:?}'"))}
            },
        };

        html! {
            h2 { (format!("Editing {component_name} of {entity}")) }
            form method="post" {
                (content_editor)
            }
        }
    }

    pub fn component_inline_view(component: Option<DynComponent>) -> maud::Markup {
        let Some(component) = component else {
            return html!(pre { "<missing>" });
        };

        match component.kind() {
            ecsdb::dyn_component::Kind::Json => {
                let Some(component_json) = component.as_json() else {
                    panic!()
                };

                let json = serde_json::to_string(&component_json).expect("component -> json");

                html! {
                    pre { (json) }
                }
            }
            ecsdb::dyn_component::Kind::Blob => {
                html!(pre { "<blob>" })
            }
            ecsdb::dyn_component::Kind::Null => {
                html! {
                    pre { "<null>" }
                }
            }
            ecsdb::dyn_component::Kind::Other(t) => html! {
                p { (format!("Unsupported ({t:?})"))}
            },
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
                    style { "[popover] { max-width: 80vw; max-height: 80vh; overflow: auto; padding: 1rem; }" }
                    base href=(base_url) { }
                }
                body {
                    header class="navbar" {
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
