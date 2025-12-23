use std::{borrow::Cow, collections::HashMap};

use axum::response::IntoResponse;
use ecsdb::{Entity, EntityId};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::HtmxTemplate;

pub trait Template {
    fn list_template() -> &'static str;
    fn detail_template() -> &'static str;

    fn default_sort_column(_entities: &[ecsdb::Entity]) -> Option<Cow<'static, str>> {
        None
    }

    fn sortable_fields<'a>(_entities: &'a [ecsdb::Entity]) -> impl IntoIterator<Item = &'a str> {
        ["id"].into_iter()
    }

    fn compare(column: &str, a: Entity, b: Entity) -> Option<std::cmp::Ordering> {
        match column {
            "id" => a.id().partial_cmp(&b.id()),
            _ => None,
        }
    }

    fn bulk_action_links() -> serde_json::Value {
        json!({})
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(try_from = "&str", into = "String")]
pub enum OrderBy {
    Asc(String),
    Desc(String),
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ListFilter {
    pub order_by: Option<OrderBy>,
    pub after: Option<EntityId>,
    #[serde(
        default,
        deserialize_with = "super::deserialize_comma_separated_string"
    )]
    pub tags: Vec<String>,
    #[serde(default)]
    pub collection: CollectionFilter,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone)]
#[serde(untagged, try_from = "String", into = "String")]
pub enum CollectionFilter {
    #[default]
    None,
    Any,
    Collection(String),
}

impl CollectionFilter {
    fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

impl TryFrom<String> for CollectionFilter {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "" => Ok(CollectionFilter::None),
            "*" => Ok(CollectionFilter::Any),
            s => Ok(CollectionFilter::Collection(s.to_string())),
        }
    }
}

impl From<CollectionFilter> for String {
    fn from(value: CollectionFilter) -> Self {
        value.to_string()
    }
}

impl std::fmt::Display for CollectionFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectionFilter::None => write!(f, ""),
            CollectionFilter::Any => write!(f, "*"),
            CollectionFilter::Collection(s) => write!(f, "{s}"),
        }
    }
}

impl std::fmt::Display for ListFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            &[
                self.after.as_ref().map(|p| format!("after={p}")),
                self.order_by.as_ref().map(|p| p.to_string()),
                (!self.tags.is_empty()).then_some(format!("tags={}", self.tags.join(","))),
                (!self.collection.is_none()).then_some(format!("collection={}", self.collection)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("&"),
        )
    }
}

const PAGE_SIZE: usize = 1000;

pub async fn list<T: Template, F: ecsdb::query::QueryFilter + Default>(
    db: super::ExtractDatabase,
    axum::extract::Query(filter_options): axum::extract::Query<ListFilter>,
) -> impl IntoResponse {
    let db = db.acquire().await;

    let all_entities: Vec<Entity> = db.query::<Entity, F>().collect();

    // let mut filtered_entities = all_entities
    //     .iter()
    //     .copied()
    //     .filter(|c| match filter_options.collection {
    //         CollectionFilter::Any => true,
    //         CollectionFilter::None => !c.has::<Collection>(),
    //         CollectionFilter::Collection(ref coll) => {
    //             c.component().is_some_and(|Collection(ref c)| c == coll)
    //         }
    //     })
    //     .filter(|c| {
    //         let entity_tags = c.component::<Tags>().unwrap_or_default();
    //         filter_options
    //             .tags
    //             .iter()
    //             .all(|ft| entity_tags.0.contains(ft))
    //     })
    //     .collect::<Vec<_>>();

    let order_by = filter_options.order_by.clone().unwrap_or(OrderBy::Desc(
        T::default_sort_column(&all_entities)
            .unwrap_or("id".into())
            .into(),
    ));

    let (order_by_property, order_asc) = match order_by {
        OrderBy::Asc(ref prop) => (prop, true),
        OrderBy::Desc(ref prop) => (prop, false),
    };

    all_entities.sort_by(|&a, &b| {
        let result = T::compare(order_by_property, a, b).unwrap_or_else(|| a.id().cmp(&b.id()));

        if order_asc { result } else { result.reverse() }
    });

    // let mut all_collections: Vec<(String, u64)> = all_entities
    //     .iter()
    //     .flat_map(|e| e.component::<Collection>())
    //     .map(|c| c.0)
    //     .fold(HashMap::new(), |mut collections, t| {
    //         *collections.entry(t).or_default() += 1;
    //         collections
    //     })
    //     .into_iter()
    //     .collect();

    // all_collections.sort_by(|a, b| a.0.cmp(&b.0));

    // let meta_collection_links = vec![
    //     {
    //         let params = ListFilter {
    //             collection: CollectionFilter::Any,
    //             ..filter_options.clone()
    //         };

    //         json!({
    //             "name": "Any",
    //             "form_value": "*",
    //             "href": format!("?{params}"),
    //         })
    //     },
    //     {
    //         let params = ListFilter {
    //             collection: CollectionFilter::None,
    //             ..filter_options.clone()
    //         };

    //         json!({
    //             "name": "None",
    //             "form_value": "",
    //             "href": format!("?{params}"),
    //         })
    //     },
    // ];

    // let named_collection_links = all_collections.iter().map(|(collection, count)| {
    //     let params = ListFilter {
    //         collection: CollectionFilter::Collection(collection.to_string()),
    //         ..filter_options.clone()
    //     };

    //     json!({
    //         "name": collection,
    //         "form_value": collection.to_string(),
    //         "href": format!("?{params}"),
    //         "count": count
    //     })
    // });

    // let all_collection_links = meta_collection_links
    //     .into_iter()
    //     .chain(named_collection_links)
    //     .collect::<Vec<_>>();

    // let mut all_tags: Vec<(String, u64)> = filtered_entities
    //     .iter()
    //     .flat_map(|e| e.component::<Tags>().unwrap_or_default().0.into_iter())
    //     .fold(HashMap::new(), |mut tags, t| {
    //         *tags.entry(t).or_default() += 1;
    //         tags
    //     })
    //     .into_iter()
    //     .collect();

    // all_tags.sort_by(|a, b| (a.1, a.0.as_str()).cmp(&(b.1, b.0.as_str())));

    // let all_tag_links: Vec<_> = all_tags
    //     .iter()
    //     .map(|(tag, count)| {
    //         let add_link = ListFilter {
    //             tags: filter_options
    //                 .tags
    //                 .iter()
    //                 .cloned()
    //                 .chain([tag.clone()])
    //                 .collect(),
    //             ..filter_options.clone()
    //         };

    //         let remove_link = ListFilter {
    //             tags: filter_options
    //                 .tags
    //                 .iter()
    //                 .filter(|&t| tag != t)
    //                 .cloned()
    //                 .collect(),
    //             ..filter_options.clone()
    //         };

    //         json!({
    //             "name": tag,
    //             "count": count,
    //             "links": {
    //                 "add": format!("?{add_link}"),
    //                 "remove": format!("?{remove_link}"),
    //             }
    //         })
    //     })
    //     .collect();

    // let multi_geometry_link = geometries::multi_geometry_link(
    //     filtered_entities
    //         .iter()
    //         .map(|e| e.id())
    //         .collect::<Vec<_>>()
    //         .as_slice(),
    // );

    let page_entities: Vec<_> = if let Some(after) = filter_options.after {
        all_entities
            .into_iter()
            .skip_while(|e| e.id() != json!(after))
            .skip(1)
            .take(PAGE_SIZE)
            .collect()
    } else {
        all_entities.truncate(PAGE_SIZE);
        all_entities
    };

    let order_by_links = ["id"]
        .into_iter()
        .chain(T::sortable_fields(&all_entities).into_iter())
        .map(|p| {
            let order_by = match &filter_options.order_by {
                Some(OrderBy::Asc(q)) if p == *q => OrderBy::Desc(p.to_string()),
                _ => OrderBy::Asc(p.to_string()),
            };

            let params = ListFilter {
                order_by: Some(order_by),
                ..filter_options.clone()
            };

            let link = format!("?{params}");

            (p, link)
        })
        .collect::<HashMap<_, _>>();

    let order_by = match order_by {
        OrderBy::Asc(p) => json!({"property": p, "direction": "asc"}),
        OrderBy::Desc(p) => json!({"property": p, "direction": "desc"}),
    };

    let next_page = if page_entities.len() >= PAGE_SIZE {
        page_entities.last().map(|last| {
            let params = ListFilter {
                after: Some(last.id()),
                ..filter_options.clone()
            };
            format!("?{params}")
        })
    } else {
        None
    };

    let self_link = {
        let params = ListFilter {
            after: None,
            ..filter_options.clone()
        };
        format!("?{params}")
    };

    let unfiltered_link = {
        let params = ListFilter {
            tags: vec![],
            collection: CollectionFilter::default(),
            after: None,
            ..filter_options.clone()
        };
        format!("?{params}")
    };

    let page_entities: Vec<_> = page_entities.into_iter().map(|e| e.id()).collect();

    HtmxTemplate {
        template_name: T::list_template(),
        context: json!({
            "entities": page_entities,
            // "collections": all_collections.iter().map(|(coll, _)| coll).collect::<Vec<_>>(),
            "order_by": order_by,
            "filter": filter_options,
            "links": {
                "self": self_link,
                "base": unfiltered_link,
                "next": next_page,
                "order_by": order_by_links,
                // "tags": all_tag_links,
                // "collections": all_collection_links,
                // "geometry": multi_geometry_link,
            }
        }),
    }
}

impl std::fmt::Display for OrderBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderBy::Asc(p) => write!(f, "order_by={p}"),
            OrderBy::Desc(p) => write!(f, "order_by=-{p}"),
        }
    }
}

impl TryFrom<&str> for OrderBy {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.chars().next() {
            Some('+') => Ok(OrderBy::Asc(value[1..].to_string())),
            Some('-') => Ok(OrderBy::Desc(value[1..].to_string())),
            Some(_) => Ok(OrderBy::Asc(value.to_string())),
            _ => Err("Invalid OrderBy"),
        }
    }
}

impl From<OrderBy> for String {
    fn from(val: OrderBy) -> Self {
        match val {
            OrderBy::Asc(p) => format!("+{p}"),
            OrderBy::Desc(p) => format!("-{p}"),
        }
    }
}
