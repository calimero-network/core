//! A cluster's nodes, from the Calimero Cloud node directory.
//!
//! `GET {directory}/{cluster}` (no auth) answers
//! `{"cluster", "nodes": [{"id", "url", "zone", "weight"}], "refreshAfterSeconds"}`,
//! listing only the nodes that should take traffic. The directory is not
//! trusted for security: every node it names is attested under the caller's
//! own policy before anything is sent to it, so a bad list can only make
//! connecting fail. Entries that are not usable (not an http(s) URL, a weight
//! that is not a positive integer) are dropped.

use std::time::Duration;

use serde_json::Value;

/// Where [`Client.connect`](super::PyCloudClient::connect) looks a cluster up.
pub(crate) const DEFAULT_DIRECTORY: &str =
    "https://manager.cloud.calimero.network/api/v2/directory";

/// How long a listing is used before it is fetched again, when the directory
/// does not say (and the bounds on what it may say).
const REFRESH_AFTER: Duration = Duration::from_secs(60);
const REFRESH_MIN: Duration = Duration::from_secs(1);
const REFRESH_MAX: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a directory request may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// One node the directory lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) url: String,
    pub(crate) zone: Option<String>,
    pub(crate) weight: u64,
}

/// A directory's answer: its usable nodes, and when to ask again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) nodes: Vec<Entry>,
    pub(crate) refresh_after: Duration,
}

/// Why the directory gave no usable node: `status` is the HTTP status it
/// answered, 0 when it could not be reached or its answer was not JSON.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DirectoryError {
    pub(crate) status: u16,
    pub(crate) message: String,
}

impl std::fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The URL that lists `cluster`'s nodes.
pub(crate) fn listing_url(directory: &str, cluster: &str) -> String {
    format!(
        "{}/{}",
        directory.trim_end_matches('/'),
        crate::convert::seg(cluster)
    )
}

/// A directory's answer, usable entries only (a URL listed twice counts once).
/// An answer with none is an error: there is nothing to connect to.
pub(crate) fn parse(cluster: &str, body: &Value) -> Result<Listing, DirectoryError> {
    let mut nodes: Vec<Entry> = Vec::new();
    for n in body["nodes"].as_array().into_iter().flatten() {
        let Some(entry) = entry(n) else { continue };
        if !nodes.iter().any(|e| e.url == entry.url) {
            nodes.push(entry);
        }
    }
    if nodes.is_empty() {
        return Err(DirectoryError {
            status: 200,
            message: format!("the directory lists no usable node for cluster '{cluster}'"),
        });
    }
    let refresh_after = body["refreshAfterSeconds"]
        .as_u64()
        .map_or(REFRESH_AFTER, Duration::from_secs)
        .clamp(REFRESH_MIN, REFRESH_MAX);
    Ok(Listing {
        nodes,
        refresh_after,
    })
}

/// One listed node, if it is usable: an http(s) URL with a host, and a
/// weight that is a positive integer.
fn entry(n: &Value) -> Option<Entry> {
    let url = n["url"].as_str()?;
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return None;
    }
    let weight = n["weight"]
        .as_u64()
        .filter(|w| (1..=u64::from(u32::MAX)).contains(w))?;
    Some(Entry {
        url: url.trim_end_matches('/').to_owned(),
        zone: n["zone"].as_str().map(str::to_owned),
        weight,
    })
}

/// The order to use `nodes` in: those in `zone` first (when given), then the
/// rest; within each group, a weighted random order (a node of weight 200 is
/// first twice as often as one of 100). `random` gives the randomness.
pub(crate) fn order(
    nodes: Vec<Entry>,
    zone: Option<&str>,
    random: &mut dyn FnMut() -> u64,
) -> Vec<String> {
    let (near, far): (Vec<_>, Vec<_>) = nodes
        .into_iter()
        .partition(|e| zone.is_some() && e.zone.as_deref() == zone);
    let mut out = weighted(near, random);
    out.extend(weighted(far, random));
    out
}

/// `group` in a weighted random order: each next node is drawn with a
/// chance proportional to its weight among those left.
fn weighted(mut group: Vec<Entry>, random: &mut dyn FnMut() -> u64) -> Vec<String> {
    let mut out = Vec::with_capacity(group.len());
    while !group.is_empty() {
        let total: u64 = group.iter().map(|e| e.weight).sum();
        let mut draw = random() % total;
        let mut pick = group.len() - 1;
        for (i, e) in group.iter().enumerate() {
            if draw < e.weight {
                pick = i;
                break;
            }
            draw -= e.weight;
        }
        out.push(group.remove(pick).url);
    }
    out
}

/// Randomness for [`order`] outside the tests.
pub(crate) fn random() -> u64 {
    u64::from_le_bytes(calimero_identity::random::<8>())
}

/// Fetch and [`parse`] `cluster`'s listing from `directory`.
pub(crate) async fn fetch(
    directory: &str,
    cluster: &str,
    roots: &super::tls::ExtraRoots,
) -> Result<Listing, DirectoryError> {
    let url = listing_url(directory, cluster);
    let failed = |status: u16, why: String| DirectoryError {
        status,
        message: format!("could not list cluster '{cluster}' from {url}: {why}"),
    };
    let http = roots
        .builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|e| failed(0, e.to_string()))?;
    let answer = http
        .get(&url)
        .send()
        .await
        .map_err(|e| failed(0, e.to_string()))?;
    let status = answer.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(DirectoryError {
            status: 404,
            message: format!("the directory at {directory} knows no cluster '{cluster}'"),
        });
    }
    if !status.is_success() {
        return Err(failed(status.as_u16(), format!("answered {status}")));
    }
    let body: Value = answer
        .json()
        .await
        .map_err(|e| failed(0, format!("the answer is not JSON: {e}")))?;
    parse(cluster, &body)
}

/// The directory a client made by `Client.connect` learns its nodes from.
pub(crate) struct Remote {
    pub(crate) directory: String,
    pub(crate) cluster: String,
    pub(crate) zone: Option<String>,
    /// The client's policy, every node a listing adds is attested under.
    pub(crate) policy: calimero_client::Policy,
    /// The roots trusted on top of the public ones (`ca_bundle`), for the
    /// directory and every node it lists.
    pub(crate) roots: super::tls::ExtraRoots,
}

impl Remote {
    /// The cluster's nodes in the order to use them, and when to ask again.
    pub(crate) async fn listing(&self) -> Result<(Vec<String>, Duration), DirectoryError> {
        let listing = fetch(&self.directory, &self.cluster, &self.roots).await?;
        let mut draw = random;
        let urls = order(listing.nodes, self.zone.as_deref(), &mut draw);
        Ok((urls, listing.refresh_after))
    }
}

impl super::recover::Directory<super::sealed::SealedNode> for Remote {
    fn fetch(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<super::recover::Listing, String>> + Send + '_>,
    > {
        Box::pin(async move { self.listing().await.map_err(|e| e.message) })
    }

    fn connect(&self, url: &str) -> Result<super::sealed::SealedNode, String> {
        super::sealed::SealedNode::connect(url, self.policy.clone(), &self.roots)
            .map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn node(url: &str, zone: &str, weight: u64) -> Entry {
        Entry {
            url: url.to_owned(),
            zone: Some(zone.to_owned()),
            weight,
        }
    }

    /// Randomness that returns `draws` in turn.
    fn draws(draws: Vec<u64>) -> impl FnMut() -> u64 {
        let mut it = draws.into_iter().cycle();
        move || it.next().unwrap()
    }

    #[test]
    fn parses_the_documented_answer() {
        let body = json!({
            "cluster": "eu-shared-1",
            "nodes": [{"id": 1, "url": "https://eu-shared-1-n1.relay.cloud.calimero.network",
                       "zone": "europe-west4-a", "weight": 100}],
            "refreshAfterSeconds": 60,
        });
        let listing = parse("eu-shared-1", &body).unwrap();
        assert_eq!(
            listing.nodes,
            vec![node(
                "https://eu-shared-1-n1.relay.cloud.calimero.network",
                "europe-west4-a",
                100
            )]
        );
        assert_eq!(listing.refresh_after, Duration::from_secs(60));
    }

    #[test]
    fn drops_unusable_entries() {
        let body = json!({ "nodes": [
            {"url": "ftp://a.example", "weight": 1},
            {"url": "not a url", "weight": 1},
            {"url": "https://zero.example", "weight": 0},
            {"url": "https://negative.example", "weight": -5},
            {"url": "https://fraction.example", "weight": 1.5},
            {"url": "https://string.example", "weight": "100"},
            {"url": "https://no-weight.example"},
            {"weight": 100},
            "https://bare-string.example",
            {"url": "https://ok.example/", "weight": 7},
            {"url": "https://ok.example", "weight": 9},
            {"url": "http://plain.example:8080", "weight": 1, "zone": null},
        ]});
        let listing = parse("c", &body).unwrap();
        assert_eq!(
            listing.nodes,
            vec![
                Entry {
                    url: "https://ok.example".into(),
                    zone: None,
                    weight: 7
                },
                Entry {
                    url: "http://plain.example:8080".into(),
                    zone: None,
                    weight: 1
                },
            ]
        );
        // No (or a bad) `refreshAfterSeconds`: the default.
        assert_eq!(listing.refresh_after, REFRESH_AFTER);
    }

    #[test]
    fn no_usable_entry_is_an_error() {
        for body in [
            json!({ "nodes": [] }),
            json!({ "nodes": [{"url": "https://a.example", "weight": 0}] }),
            json!({}),
            json!([]),
        ] {
            let e = parse("eu-shared-1", &body).unwrap_err();
            assert!(e.message.contains("no usable node"), "{e}");
            assert!(e.message.contains("eu-shared-1"), "{e}");
        }
    }

    #[test]
    fn refresh_after_is_bounded() {
        let nodes = json!([{"url": "https://a.example", "weight": 1}]);
        let at = |s: Value| {
            parse("c", &json!({ "nodes": nodes, "refreshAfterSeconds": s }))
                .unwrap()
                .refresh_after
        };
        assert_eq!(at(json!(0)), REFRESH_MIN);
        assert_eq!(at(json!(5)), Duration::from_secs(5));
        assert_eq!(at(json!(10_000_000)), REFRESH_MAX);
    }

    #[test]
    fn the_zone_comes_first_then_the_rest() {
        let nodes = vec![
            node("https://a1", "a", 1),
            node("https://b1", "b", 1),
            node("https://a2", "a", 1),
            node("https://b2", "b", 1),
        ];
        // Draw 0 always picks the first left in its group.
        let got = order(nodes.clone(), Some("b"), &mut draws(vec![0]));
        assert_eq!(
            got,
            ["https://b1", "https://b2", "https://a1", "https://a2"]
        );
        // No zone, or one nobody is in: one group.
        let got = order(nodes.clone(), None, &mut draws(vec![0]));
        assert_eq!(
            got,
            ["https://a1", "https://b1", "https://a2", "https://b2"]
        );
        let got = order(nodes, Some("z"), &mut draws(vec![0]));
        assert_eq!(
            got,
            ["https://a1", "https://b1", "https://a2", "https://b2"]
        );
    }

    #[test]
    fn the_order_within_a_group_is_drawn_by_weight() {
        let nodes = vec![
            node("https://light", "a", 100),
            node("https://heavy", "a", 300),
        ];
        // A draw below 100 lands on the first node, from 100 to 399 on the second.
        assert_eq!(
            order(nodes.clone(), None, &mut draws(vec![99])),
            ["https://light", "https://heavy"]
        );
        assert_eq!(
            order(nodes.clone(), None, &mut draws(vec![100])),
            ["https://heavy", "https://light"]
        );
        // Over many draws the heavier node leads about three times in four.
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut xorshift = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let heavy_first = (0..4000)
            .filter(|_| order(nodes.clone(), None, &mut xorshift)[0] == "https://heavy")
            .count();
        assert!((2800..3200).contains(&heavy_first), "{heavy_first}");
    }

    #[test]
    fn the_cluster_is_one_path_segment() {
        assert_eq!(
            listing_url(DEFAULT_DIRECTORY, "eu-shared-1"),
            "https://manager.cloud.calimero.network/api/v2/directory/eu-shared-1"
        );
        assert_eq!(
            listing_url("http://d.example/dir/", "../x y"),
            "http://d.example/dir/..%2Fx%20y"
        );
    }
}
