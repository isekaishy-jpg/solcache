use solcache_solworker_example::{
    forward_ordered_demand, run_pipeline, run_transfer_rejection, settle_saturated_attempt,
};

#[test]
fn worker_production_discovery_and_owner_publication_preserve_owned_bytes() {
    let report = run_pipeline();
    assert_eq!(report.bytes, [2, 4, 6, 8]);
    assert!(report.input_address_preserved);
    assert!(report.output_address_preserved);
    assert!(report.applied_on_owner);
    assert!(report.destroyed_on_owner);
}

#[test]
fn saturated_operation_can_settle_without_being_invoked() {
    settle_saturated_attempt();
}

#[test]
fn current_demand_changes_pending_resource_order() {
    forward_ordered_demand();
}

#[test]
fn rejected_output_transfer_preserves_sw_lease_bytes_and_original_publication_ticket() {
    let report = run_transfer_rejection();
    assert!(report.transfer_rejected);
    assert_eq!(report.bytes, [2, 4, 6, 8]);
    assert!(report.output_address_preserved);
    assert!(report.applied_on_owner);
    assert!(report.destroyed_on_owner);
}
