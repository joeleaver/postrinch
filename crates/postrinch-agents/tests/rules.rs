//! An agent's rules calls, over a real loopback connection.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use postrinch_agents::client::Client;
use postrinch_agents::rule_shape::RuleShape;
use postrinch_agents::{Agents, Call, Changed, Reach};
use postrinch_commands::Commands;
use postrinch_model::{Account, EmailAddress, Label, LabelKind, Provider, SystemLabel};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn add_account(store: &Store, account: &EmailAddress) {
    store
        .put_account(&Account::new(account.clone(), Provider::Google))
        .unwrap();
    store
        .put_label(
            account,
            &Label::new("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox)),
        )
        .unwrap();
    store
        .put_label(account, &Label::new("Label_1", "Receipts", LabelKind::User))
        .unwrap();
    store
        .update_rules(account, |_, local| {
            local.forwarding = vec!["books@example.org".into()];
        })
        .unwrap();
}

fn shape(json: &str) -> RuleShape {
    serde_json::from_str(json).unwrap()
}

#[test]
fn an_agent_makes_rules_and_proposes_the_ones_that_send() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("store")).unwrap();
    let work = address("work@example.org");
    let home = address("home@example.org");
    add_account(&store, &work);
    add_account(&store, &home);

    let agents_path = dir.path().join("agents.json");
    let mut agents = Agents::load(&agents_path).unwrap();
    let (_, key) = agents
        .create("Test agent", Reach::Only(vec![work.clone()]))
        .unwrap();

    let changes = Arc::new(Mutex::new(Vec::new()));
    let seen = changes.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let commands = Commands::new(store.clone());
    postrinch_agents::serve::start_on(
        listener,
        commands.clone(),
        agents_path,
        Arc::new(move |c: &Changed| seen.lock().unwrap().push(c.clone())),
    )
    .unwrap();
    let client = Client::new(addr, Some(key));

    // A rule that files mail is saved, its label named as the person sees it.
    let saved = client
        .call(&Call::SaveRule {
            account: work.clone(),
            rule_id: None,
            rule: shape(
                r#"{"name":"Receipts","conditions":[{"test":"subject_words","value":"receipt"}],
                    "actions":[{"action":"add_label","value":"receipts"},{"action":"skip_inbox"}]}"#,
            ),
            apply_to_existing: false,
            reason: None,
        })
        .unwrap();
    let id = saved["saved"].as_u64().unwrap() as u32;
    assert_eq!(
        changes.lock().unwrap().as_slice(),
        [Changed::Rules(work.clone())]
    );

    let listed = client
        .call(&Call::Rules {
            account: work.clone(),
        })
        .unwrap();
    let rule = &listed["rules"][0];
    assert_eq!(rule["id"], id);
    assert_eq!(rule["runs_in"], "postrinch");
    assert_eq!(rule["rule"]["actions"][0]["value"], "Receipts");
    assert!(
        rule["summary"].as_str().unwrap().contains("Receipts"),
        "{listed}"
    );

    // One that forwards waits for the person, with the agent's reason.
    let forward = client
        .call(&Call::SaveRule {
            account: work.clone(),
            rule_id: Some(id),
            rule: shape(
                r#"{"name":"Receipts","conditions":[{"test":"subject_words","value":"receipt"}],
                    "actions":[{"action":"forward","value":"books@example.org"}]}"#,
            ),
            apply_to_existing: false,
            reason: Some("Your bookkeeper wants them".into()),
        })
        .unwrap();
    assert_eq!(forward["proposed"], true);
    let page = commands.rules_page(&work).unwrap();
    assert_eq!(
        page.rules[0].actions.len(),
        2,
        "the saved rule is unchanged"
    );
    let waiting = commands.rule_proposals().unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].proposed.agent, "Test agent");
    assert_eq!(waiting[0].proposed.rule.id, id);
    assert_eq!(
        waiting[0].proposed.reason.as_deref(),
        Some("Your bookkeeper wants them")
    );

    // Switched off, then deleted.
    client
        .call(&Call::SetRuleOn {
            account: work.clone(),
            rule_id: id,
            on: false,
        })
        .unwrap();
    assert!(!commands.rules_page(&work).unwrap().rules[0].enabled);
    client
        .call(&Call::DeleteRule {
            account: work.clone(),
            rule_id: id,
        })
        .unwrap();
    assert!(commands.rules_page(&work).unwrap().rules.is_empty());

    // Mistakes and other accounts are refused in a sentence.
    let gone = client.call(&Call::DeleteRule {
        account: work.clone(),
        rule_id: id,
    });
    assert!(gone.unwrap_err().contains("has no rule"));
    let elsewhere = client.call(&Call::Rules {
        account: home.clone(),
    });
    assert_eq!(elsewhere.unwrap_err(), "No account home@example.org.");
    let unverified = client.call(&Call::SaveRule {
        account: work.clone(),
        rule_id: None,
        rule: shape(
            r#"{"name":"R","conditions":[{"test":"unread"}],"actions":[{"action":"forward","value":"x@example.net"}]}"#,
        ),
        apply_to_existing: false,
        reason: None,
    });
    assert!(
        unverified
            .unwrap_err()
            .contains("only to addresses verified")
    );
    let no_label = client.call(&Call::SaveRule {
        account: work.clone(),
        rule_id: None,
        rule: shape(
            r#"{"name":"R","conditions":[{"test":"unread"}],"actions":[{"action":"add_label","value":"Taxes"}]}"#,
        ),
        apply_to_existing: false,
        reason: None,
    });
    assert!(no_label.unwrap_err().contains("no label \"Taxes\""));
}
