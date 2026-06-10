//! Ported `Tap` / `Untap` observer surface, library level.
//!
//! Old spirit's `OperationKind` carried `Tap` and `Untap`: a meta-observation
//! stream that filtered `OperationReceived` events by an `ObserverFilter`
//! (`All` / `OperationsOnly` / `EffectsOnly`). The new spirit had dropped both.
//! This ports them as a request/reply observer surface: every admitted working
//! operation is recorded in a typed operation log, `Tap(ObserverFilter)` mints
//! an observer subscription and returns the operations observed so far filtered
//! by the chosen filter, and `Untap(token)` retires the subscription and
//! returns its final filtered observations. `Watch`/`Unwatch` reconciliation:
//! `SubscribeIntent` already covers old `Watch` (records subscription), so the
//! un-covered half — token-based cancellation — is what `Untap` restores.

use spirit::schema::signal::{
    CertaintySelection, Description, Entry, Input, Kind, Magnitude, ObserverFilter, OperationKind,
    Output, Privacy, PrivacySelection, Query, TopicMatch, Topics,
};
use spirit::{Engine, Store};
use tempfile::TempDir;

fn entry(description: &str) -> Entry {
    Entry {
        topics: Topics::from_strings(vec![String::from("observer-tap")]),
        kind: Kind::Decision,
        description: Description::new(description),
        magnitude: Magnitude::Maximum,
        privacy: Privacy::new(Magnitude::Zero),
    }
}

fn observe_query() -> Query {
    Query {
        topic_match: TopicMatch::full(Topics::from_strings(vec![String::from("observer-tap")])),
        kind: Some(Kind::Decision),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::default_observation_certainty(),
    }
}

fn engine() -> (TempDir, Engine) {
    let temp = TempDir::new().expect("tempdir");
    let database = temp.path().join("observer-tap.sema");
    let mut engine = Engine::new(Store::open(&database).expect("open store"));
    engine.start().expect("engine start");
    (temp, engine)
}

#[test]
fn tap_returns_the_operations_observed_so_far() {
    let (_temp, mut engine) = engine();

    // Drive a few working operations; each is recorded in the observer log.
    let _ = engine
        .handle(Input::record(entry("first intent")))
        .into_root();
    let _ = engine
        .handle(Input::record(entry("second intent")))
        .into_root();
    let _ = engine.handle(Input::observe(observe_query())).into_root();

    // Tap with the `All` filter: the reply lists the operations observed so far.
    let reply = engine.handle(Input::tap(ObserverFilter::All)).into_root();
    let Output::ObservationTapped(subscription) = reply else {
        panic!("expected ObservationTapped, got {reply:?}")
    };
    assert!(
        subscription.subscription_token >= 1,
        "the tap minted a subscription token"
    );
    assert_eq!(
        subscription.observer_filter,
        ObserverFilter::All,
        "the reply echoes the requested observer filter"
    );
    // Record, Record, Observe, then the Tap itself: four observed operations.
    let kinds: Vec<OperationKind> = subscription
        .observed_operations
        .iter()
        .map(|operation| *operation.payload())
        .collect();
    assert_eq!(
        kinds,
        vec![
            OperationKind::Record,
            OperationKind::Record,
            OperationKind::Observe,
            OperationKind::Tap,
        ],
        "the tap observed every admitted operation in order"
    );
}

#[test]
fn untap_retires_the_subscription_and_returns_its_observations() {
    let (_temp, mut engine) = engine();

    let _ = engine.handle(Input::record(entry("intent"))).into_root();
    let tapped = engine
        .handle(Input::tap(ObserverFilter::OperationsOnly))
        .into_root();
    let Output::ObservationTapped(subscription) = tapped else {
        panic!("expected ObservationTapped, got {tapped:?}")
    };
    let token = subscription.subscription_token.clone();

    let untapped = engine.handle(Input::untap(token.clone())).into_root();
    let Output::ObservationUntapped(retraction) = untapped else {
        panic!("expected ObservationUntapped, got {untapped:?}")
    };
    assert_eq!(
        retraction.subscription_token, token,
        "the retraction names the closed subscription token"
    );

    // Untapping the same token again returns an empty observation set, proving
    // the subscription was retired.
    let again = engine.handle(Input::untap(token)).into_root();
    let Output::ObservationUntapped(retraction_again) = again else {
        panic!("expected ObservationUntapped, got {again:?}")
    };
    assert!(
        retraction_again.observed_operations.is_empty(),
        "a retired subscription has no further observations"
    );
}

#[test]
fn effects_only_filter_observes_no_operations() {
    let (_temp, mut engine) = engine();

    let _ = engine.handle(Input::record(entry("intent"))).into_root();

    // `EffectsOnly` observes effect events, not operations, so an operation-only
    // log yields an empty observation set under this filter.
    let reply = engine
        .handle(Input::tap(ObserverFilter::EffectsOnly))
        .into_root();
    let Output::ObservationTapped(subscription) = reply else {
        panic!("expected ObservationTapped, got {reply:?}")
    };
    assert!(
        subscription.observed_operations.is_empty(),
        "the EffectsOnly filter observes no operation events"
    );
}
