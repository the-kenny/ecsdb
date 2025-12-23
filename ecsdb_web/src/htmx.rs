use axum::{
    extract,
    response::{IntoResponse, Response},
};
use ecsdb::{Component, Entity, EntityId, query};
use http::{HeaderValue, header};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, debug_span};

use super::entity_context;

#[derive(Clone)]
pub struct HtmxTemplate {
    pub template_name: &'static str,
    pub context: serde_json::Value,
}

impl HtmxTemplate {
    pub fn new(template_name: &'static str, entity: Entity) -> Self {
        let context = entity_context(entity);

        Self {
            template_name,
            context,
        }
    }
}

impl IntoResponse for HtmxTemplate {
    fn into_response(self) -> Response<axum::body::Body> {
        Response::builder()
            .extension(self)
            .body(axum::body::Body::empty())
            .unwrap()
    }
}

tokio::task_local! {
    static DB: ecsdb::Ecs;
}

#[derive(Debug, Copy, Clone)]
pub struct TemplateEcs;

impl minijinja::value::Object for TemplateEcs {
    fn call_method(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[minijinja::Value],
    ) -> Result<minijinja::Value, minijinja::Error> {
        DB.with(|db| match (method, &args) {
            ("query", component_names) => {
                let _span = debug_span!("query", ?component_names).entered();

                let component_names: Vec<query::ComponentName> = component_names
                    .iter()
                    .filter_map(|name| name.as_str())
                    .map(|name| query::ComponentName(name.to_owned()))
                    .collect();

                let entities = db
                    .query_filtered::<Entity, ()>(&component_names[..])
                    .map(|e| TemplateEntity(e.id()))
                    .map(minijinja::Value::from_object)
                    .collect::<Vec<_>>();

                debug!(?entities);

                Ok(entities.into())
            }

            (other, args) => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!(
                    "{}({})",
                    other,
                    args.iter()
                        .map(|v| v.kind().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct TemplateEntity(pub EntityId);

impl TemplateEntity {
    fn similarity(self, db: &ecsdb::Ecs) -> Vec<(EntityId, similarity::SimilarityFactor)> {
        let mut similarity: Vec<_> = db
            .entity(self.0)
            .component::<similarity::Similarity>()
            .unwrap_or_default()
            .0
            .into_iter()
            .collect();

        similarity.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
        similarity
    }
}

impl minijinja::value::Object for TemplateEntity {
    fn is_true(self: &std::sync::Arc<Self>) -> bool {
        DB.with(|db| db.entity(self.0).exists())
    }

    fn get_value(
        self: &std::sync::Arc<Self>,
        field: &minijinja::Value,
    ) -> Option<minijinja::Value> {
        DB.with(|db| {
            db.entity(self.0)
                .dyn_component(field.as_str()?)
                .and_then(|c| c.as_json())
                .map(minijinja::Value::from_serialize)
        })
    }

    fn call_method(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[minijinja::Value],
    ) -> Result<minijinja::Value, minijinja::Error> {
        use minijinja::{Value, value::ValueKind};

        DB.with(|db| match (method, &args) {
            ("exists", &[]) => Ok(db.entity(self.0).or_none().is_some().into()),
            ("component", &[name]) if name.kind() == ValueKind::String => {
                Ok(self.get_value(name).unwrap_or_default())
            }
            ("components", &[]) => Ok(db
                .entity(self.0)
                .component_names()
                .collect::<Vec<_>>()
                .into()),
            ("id", &[]) => Ok(Value::from(self.0)),
            ("has", &[name]) if name.kind() == ValueKind::String => {
                let name = name.as_str().unwrap();
                Ok(Value::from(
                    db.entity(self.0).component_names().any(|c| c == name),
                ))
            }

            ("kind", &[]) => Ok(Value::from(
                VelodbEntity::classify(db.entity(self.0)).as_str(),
            )),
            ("last_modified", &[]) => {
                Ok(Value::from(db.entity(self.0).last_modified().to_rfc3339()))
            }
            ("ref", &[component]) if component.kind() == ValueKind::String => {
                let Some(component) = db.entity(self.0).dyn_component(component.as_str().unwrap())
                else {
                    return Ok(Value::default());
                };

                #[derive(Deserialize, Serialize, Component)]
                struct RefComponent(EntityId);

                match component.as_typed() {
                    Ok(RefComponent(eid)) => Ok(Value::from_object(TemplateEntity(eid))),
                    Err(e) => Err(minijinja::Error::new(
                        minijinja::ErrorKind::CannotDeserialize,
                        format!("{} not a referencing component: {e}", component.name()),
                    )),
                }
            }
            ("self_link", &[]) => {
                let link = super::self_link(VelodbEntity::classify(db.entity(self.0)), self.0);
                Ok(link.into())
            }
            ("similarity", &[]) => Ok(Value::from(
                self.similarity(db)
                    .into_iter()
                    .filter(|(eid, _)| db.entity(*eid).exists())
                    .map(|(eid, sim)| {
                        vec![Value::from_object(TemplateEntity(eid)), Value::from(sim)]
                    })
                    .collect::<Vec<_>>(),
            )),
            ("tags", &[]) => {
                let entity = db.entity(self.0);
                let tags = entity.component::<crate::Tags>().unwrap_or_default();

                Ok(minijinja::Value::from_serialize(tags))
            }
            ("geometry_link", &[]) => {
                let entity = db.entity(self.0);
                let link = Geometry::link(entity);
                Ok(minijinja::Value::from_serialize(link))
            }
            (other, args) => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!(
                    "{}({})",
                    other,
                    args.iter()
                        .map(|v| v.kind().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        })
    }
}

pub async fn htmx_middleware(
    app_state: extract::State<AppState<'static>>,
    user_session: Option<UserSession>,
    db: Option<super::ExtractDatabase>,
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_hx_request = request
        .headers()
        .get("HX-Request")
        .is_some_and(|h| h.to_str().is_ok_and(|v| v == "true"));
    let is_hx_boosted = request
        .headers()
        .get("HX-Boosted")
        .is_some_and(|h| h.to_str().is_ok_and(|v| v == "true"));

    let is_htmx_request = is_hx_request && !is_hx_boosted;

    // let app_state = request.extensions().get::<AppState>().unwrap().to_owned();
    let mut response = next.run(request).await;

    let Some(HtmxTemplate {
        template_name,
        mut context,
    }) = response.extensions_mut().remove::<HtmxTemplate>()
    else {
        return response;
    };

    // Enrich `context` with user-info
    match user_session {
        None => {}
        Some(UserSession::OAuth2 { user_id, email }) => {
            context["user"] = json!({
                "id": user_id,
                "email": email
            });
        }
        Some(UserSession::ApiToken { user_id, token }) => {
            context["user"] = json!({
                "id": user_id,
                "api_token": token,
            });
        }
    }

    let run = || {
        let body = if is_htmx_request {
            let template = app_state.templates.get_template(template_name).unwrap();

            let mut state = template.eval_to_state(&context).unwrap();
            match state.render_block("content") {
                Ok(x) => x,
                Err(e) if e.kind() == minijinja::ErrorKind::UnknownBlock => {
                    template.render(context).unwrap()
                }
                Err(e) => panic!("{e}"),
            }
        } else {
            app_state.render_template(template_name, context)
        };

        // Fix Content-Length
        response.headers_mut().remove(header::CONTENT_LENGTH);
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));

        *response.body_mut() = axum::body::Body::new(body);

        // Default to HTML
        response
            .headers_mut()
            .entry(header::CONTENT_TYPE)
            .or_insert(http::HeaderValue::from_static(
                mime::TEXT_HTML_UTF_8.as_ref(),
            ));

        response
    };

    if let Some(db) = db {
        DB.sync_scope(db.acquire().await, run)
    } else {
        run()
    }
}
