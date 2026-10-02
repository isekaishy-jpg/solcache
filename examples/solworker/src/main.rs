fn main() {
    let report = solcache_solworker_example::run_pipeline();
    solcache_solworker_example::settle_saturated_attempt();
    solcache_solworker_example::forward_ordered_demand();
    println!(
        "Published {:?}; retained retry, discovery, demand and owner cleanup settled.",
        report.bytes
    );
}
