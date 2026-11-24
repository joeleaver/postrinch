//! Real discovery over the network, run by hand:
//! `cargo test -p postrinch-discover --test live -- --ignored --nocapture`.

use postrinch_discover::{Discoverer, Security, Source, discover};

#[tokio::test]
#[ignore = "uses the network; run by hand"]
async fn fastmail_for_real() {
    let d = Discoverer::new().expect("the real discoverer");
    let found = discover("someone@fastmail.com", &d, |p| println!("{p:?}"))
        .await
        .expect("a valid address");
    println!("{found:#?}");
    assert!(matches!(
        found.source,
        Source::DomainAutoconfig | Source::Ispdb
    ));
    assert_eq!(found.imap[0].host, "imap.fastmail.com");
    assert_eq!(found.imap[0].security, Security::Tls);
    assert!(!found.smtp.is_empty());
}
