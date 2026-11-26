use super::*;

#[test]
fn handles() {
    assert!(is_valid_handle("alice"));
    assert!(is_valid_handle("a_b-9"));
    assert!(!is_valid_handle("al"));
    assert!(!is_valid_handle("Alice"));
    assert!(!is_valid_handle(&"a".repeat(33)));
    assert!(!is_valid_handle("al.ice"));
}

#[test]
fn canonical_authorities() {
    for ok in [
        "providera.com",
        "localhost:8080",
        "xn--bcher-kva.example",
        "a.b-c.d",
    ] {
        assert_eq!(Authority::parse_canonical(ok).unwrap().as_str(), ok);
    }
    for (input, want) in [
        ("ProviderA.com", "providera.com"),
        ("providera.com.", "providera.com"),
        ("providera.com:443", "providera.com"),
        ("PROVIDERA.COM.:443", "providera.com"),
        ("providera.com:8443", "providera.com:8443"),
    ] {
        if input != want {
            assert_eq!(
                Authority::parse_canonical(input),
                Err(IdentError::NotCanonical)
            );
        }
        assert_eq!(Authority::canonicalize(input).unwrap().as_str(), want);
    }
    for bad in [
        "",
        "bücher.example",
        "a..b",
        "-a.com",
        "a-.com",
        "a.com:",
        "a.com:08080",
        "a.com:99999",
        "a.com:+80",
        "[::1]:8080",
        "a_b.com",
        "a.com/x",
    ] {
        assert_eq!(
            Authority::canonicalize(bad),
            Err(IdentError::Authority),
            "{bad}"
        );
    }
}

#[test]
fn actors() {
    let a = Actor::parse_canonical("alice@providera.com").unwrap();
    assert_eq!(a.handle(), "alice");
    assert_eq!(a.domain().as_str(), "providera.com");
    assert_eq!(a.to_string(), "alice@providera.com");

    assert_eq!(
        Actor::parse_canonical("alice@ProviderA.com"),
        Err(IdentError::NotCanonical)
    );
    assert_eq!(Actor::canonicalize("alice@ProviderA.com:443").unwrap(), a);
    // A handle is never lowercased (§2.2.1).
    assert_eq!(
        Actor::canonicalize("Alice@providera.com"),
        Err(IdentError::Handle)
    );
    assert_eq!(Actor::parse_canonical("alice"), Err(IdentError::MissingAt));
    assert_eq!(
        Actor::parse_canonical("alice@bob@x.com"),
        Err(IdentError::Authority)
    );
}

#[test]
fn same_handle_on_two_providers_is_two_actors() {
    let local = Actor::parse_canonical("alice@a.example").unwrap();
    let remote = Actor::parse_canonical("alice@b.example").unwrap();
    assert_ne!(local, remote);
    let home = Authority::parse_canonical("a.example").unwrap();
    assert!(local.is_local_to(&home));
    assert!(!remote.is_local_to(&home));
}

#[test]
fn actor_serde_canonicalizes() {
    let a: Actor = serde_json::from_str("\"bob@B.Example.\"").unwrap();
    assert_eq!(serde_json::to_string(&a).unwrap(), "\"bob@b.example\"");
    assert!(serde_json::from_str::<Actor>("\"Bob@b.example\"").is_err());
}

#[test]
fn local_ids() {
    for ok in ["grp_1", "A-z_09", &"x".repeat(128)] {
        assert_eq!(LocalId::new(ok).unwrap().as_str(), ok);
    }
    for bad in ["", "a/b", "a b", "a.b", "a%2F", &"x".repeat(129)] {
        assert_eq!(LocalId::new(bad), Err(IdentError::LocalId), "{bad}");
    }
    assert!(serde_json::from_str::<LocalId>("\"../etc\"").is_err());
}

#[test]
fn object_uris() {
    let uri = ObjectUri::parse("https://A.example:443/api/groups/grp_1").unwrap();
    assert_eq!(uri.authority().as_str(), "a.example");
    assert_eq!(uri.path(), "/api/groups/grp_1");
    assert_eq!(uri.to_string(), "https://a.example/api/groups/grp_1");

    let bare = ObjectUri::parse("https://a.example:8443").unwrap();
    assert_eq!(bare.to_string(), "https://a.example:8443");

    for bad in [
        "http://a.example/x",
        "https://",
        "https:///x",
        "a.example/x",
        "https://bad_host/x",
    ] {
        assert_eq!(ObjectUri::parse(bad), Err(IdentError::ObjectUri), "{bad}");
    }
}

#[test]
fn refs_are_local_ids_or_uris() {
    assert_eq!(
        ObjectRef::parse("chn_general").unwrap(),
        ObjectRef::Local(LocalId::new("chn_general").unwrap())
    );
    let uri = "https://b.example/api/groups/grp_2/channels/chn_9";
    match ObjectRef::parse(uri).unwrap() {
        ObjectRef::Uri(u) => assert_eq!(u.to_string(), uri),
        other => panic!("expected a URI, got {other:?}"),
    }
    assert!(ObjectRef::parse("not a ref").is_err());
}
