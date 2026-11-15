//! Case-insensitive routing, as Jellyfin's ASP.NET routes match. The router itself is
//! case-sensitive, so each request path is first rewritten to the case of the route it matches
//! without case: `/users/public` becomes `/Users/Public`. A literal segment beats a parameter, as
//! in ASP.NET (`/Users/me` is `/Users/Me`, not a user id); parameters keep their case. A path that
//! matches no route, or matches it exactly, is left alone, so the web client's files under `/web`
//! keep their case-sensitive names.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use axum::extract::Request;
use axum::http::Uri;

include!(concat!(env!("OUT_DIR"), "/route_templates.rs"));

#[derive(Default)]
struct Node {
    literals: BTreeMap<String, Node>,
    param: Option<Box<Node>>,
    wildcard: bool,
    end: bool,
}

impl Node {
    fn insert(&mut self, template: &str) {
        let mut node = self;
        for seg in template.split('/').skip(1) {
            if seg.starts_with("{*") {
                node.wildcard = true;
                return;
            }
            node = if seg.starts_with('{') && seg.ends_with('}') {
                node.param.get_or_insert_with(Default::default)
            } else {
                node.literals.entry(seg.to_string()).or_default()
            };
        }
        node.end = true;
    }

    /// Fills `out` with the segments of the route `segs` matches, trying an exact literal, then a
    /// literal without case, then a parameter, then a catch-all.
    fn resolve<'a>(&'a self, segs: &[&'a str], out: &mut Vec<&'a str>) -> bool {
        let Some((seg, rest)) = segs.split_first() else {
            return self.end;
        };
        if let Some(child) = self.literals.get(*seg) {
            out.push(seg);
            if child.resolve(rest, out) {
                return true;
            }
            out.pop();
        }
        for (name, child) in &self.literals {
            if name != seg && name.eq_ignore_ascii_case(seg) {
                out.push(name);
                if child.resolve(rest, out) {
                    return true;
                }
                out.pop();
            }
        }
        if let Some(child) = &self.param {
            out.push(seg);
            if child.resolve(rest, out) {
                return true;
            }
            out.pop();
        }
        if self.wildcard {
            out.extend_from_slice(segs);
            return true;
        }
        false
    }
}

static ROUTES: LazyLock<Node> = LazyLock::new(|| {
    let mut root = Node::default();
    for template in ROUTE_TEMPLATES {
        root.insert(template);
    }
    root
});

/// The path in the case of the route it matches, or `None` when it needs no change.
pub(crate) fn canonical_path(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.split('/').skip(1).collect();
    let mut out = Vec::with_capacity(segs.len());
    if !path.starts_with('/') || !ROUTES.resolve(&segs, &mut out) {
        return None;
    }
    let canonical = format!("/{}", out.join("/"));
    (canonical != path).then_some(canonical)
}

/// Rewrites the request's path to its route's case (see the module docs).
pub(crate) async fn canonical_case(mut req: Request) -> Request {
    if let Some(path) = canonical_path(req.uri().path()) {
        let path_and_query = match req.uri().query() {
            Some(q) => format!("{path}?{q}"),
            None => path,
        };
        let mut parts = req.uri().clone().into_parts();
        if let Ok(pq) = path_and_query.parse() {
            parts.path_and_query = Some(pq);
            if let Ok(uri) = Uri::from_parts(parts) {
                *req.uri_mut() = uri;
            }
        }
    }
    req
}

#[cfg(test)]
mod tests {
    use super::canonical_path;

    #[test]
    fn r1auth_literals_match_without_case() {
        assert_eq!(
            canonical_path("/users/public").as_deref(),
            Some("/Users/Public")
        );
        assert_eq!(
            canonical_path("/Users/authenticatebyname").as_deref(),
            Some("/Users/AuthenticateByName")
        );
        assert_eq!(
            canonical_path("/ITEMS/abc/IMAGES/Primary").as_deref(),
            Some("/Items/abc/Images/Primary")
        );
        assert_eq!(
            canonical_path("/system/info/public").as_deref(),
            Some("/System/Info/Public")
        );
    }

    #[test]
    fn r1auth_exact_and_unknown_paths_are_left_alone() {
        assert_eq!(canonical_path("/Users/Public"), None);
        assert_eq!(canonical_path("/web/index.html"), None);
        assert_eq!(canonical_path("/web/assets/Logo-AbC12345.js"), None);
        assert_eq!(canonical_path("/no/such/route"), None);
        assert_eq!(canonical_path(""), None);
    }

    #[test]
    fn r1auth_literal_beats_parameter_and_params_keep_case() {
        assert_eq!(canonical_path("/users/me").as_deref(), Some("/Users/Me"));
        // No /Users/Me/Items route, so "me" is a user id there and keeps its case.
        assert_eq!(
            canonical_path("/users/me/items").as_deref(),
            Some("/Users/me/Items")
        );
        assert_eq!(
            canonical_path("/genres/Science Fiction").as_deref(),
            Some("/Genres/Science Fiction")
        );
        assert_eq!(canonical_path("/Genres/items"), None);
    }
}
