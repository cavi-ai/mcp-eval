use mcpeval::measurement::MeasurementProfile;
use mcpeval::score::{fold, Area, CheckId, CheckReason};
use mcpeval::standard::{Observations, StandardOptions, REPEATS};

#[test]
fn current_profile_accepts_the_three_argument_api() {
    let digest = "a".repeat(64);
    let profile = MeasurementProfile::current(true, &["b".into(), "a".into()], Some(&digest));

    assert!(profile.attested_read_only);
    assert_eq!(profile.skip_tools, ["a", "b"]);
    assert_eq!(profile.arguments_sha256.as_deref(), Some(digest.as_str()));
    assert_eq!(profile.repeats, REPEATS);
    assert!(!profile.sandboxed);
    assert!(!profile.sandbox_network);
}

#[test]
fn fold_accepts_optional_boolean_server_results() {
    let readiness = fold(&Observations {
        tools: Vec::new(),
        attested_read_only: false,
        sandboxed: false,
        contention: Some(false),
        payload: None,
        protocol: Vec::new(),
    });
    let reliability = readiness
        .areas
        .iter()
        .find(|area| area.area == Area::Reliability)
        .unwrap();

    assert!(reliability.checks.iter().any(|check| {
        check.id == CheckId::ReliabilityContention
            && check.reason == CheckReason::ReliabilityContentionFailed
    }));
    assert!(!reliability
        .checks
        .iter()
        .any(|check| check.id == CheckId::ReliabilityPayload));
}

#[test]
fn standard_options_accepts_the_public_struct_fields() {
    let options = StandardOptions {
        confirm_read_only: false,
        skip_tools: Vec::new(),
        arguments: None,
        sandboxed: true,
        sandbox_network: true,
        repeats: REPEATS,
    };

    assert!(options.sandboxed);
    assert!(options.sandbox_network);
}
