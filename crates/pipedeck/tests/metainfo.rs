//! What software centres read of Pipedeck, kept in step with the release.

const METAINFO: &str = include_str!("../data/dev._2c2t.Pipedeck.metainfo.xml");

#[test]
fn the_newest_release_is_this_version() {
    let newest = METAINFO
        .split("<release version=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the metainfo lists its releases");
    assert_eq!(
        newest,
        env!("CARGO_PKG_VERSION"),
        "add this version at the top of the metainfo's releases"
    );
}
