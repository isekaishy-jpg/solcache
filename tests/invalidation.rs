use solcache::{SCDependencies, SCDependencyError, SCDependencyScope};

#[test]
fn selected_propagation_preserves_independent_scopes() {
    let mut dependencies = SCDependencies::new();
    let source = dependencies.register(SCDependencyScope::Source);
    let representation = dependencies.register(SCDependencyScope::Representation);
    let instance = dependencies.register(SCDependencyScope::Instance);
    let view = dependencies.register(SCDependencyScope::View);
    let device = dependencies.register(SCDependencyScope::Device);
    let unaffected = dependencies
        .snapshot(&[source.clone(), instance.clone(), view.clone()])
        .unwrap();
    let gpu = dependencies
        .snapshot(&[representation.clone(), device.clone()])
        .unwrap();
    let clone = gpu.clone();

    let current = dependencies
        .advance(&[device.clone(), representation.clone(), device])
        .unwrap();
    assert_eq!(current.len(), 2);
    assert_eq!(dependencies.validate(&current), Ok(()));
    assert_eq!(dependencies.validate(&gpu), Err(SCDependencyError::Stale));
    assert_eq!(dependencies.validate(&clone), Err(SCDependencyError::Stale));
    assert_eq!(dependencies.validate(&unaffected), Ok(()));
    assert_eq!(source.scope(), SCDependencyScope::Source);

    // The domain explicitly declares source-to-representation propagation.
    dependencies.advance(&[source, representation]).unwrap();
    assert_eq!(
        dependencies.validate(&unaffected),
        Err(SCDependencyError::Stale)
    );
    assert_eq!(
        dependencies.validate(&current),
        Err(SCDependencyError::Stale)
    );
}

#[test]
fn invalid_selected_inputs_leave_every_revision_unchanged() {
    let mut registry = SCDependencies::new();
    let valid = registry.register(SCDependencyScope::Source);
    let removed = registry.register(SCDependencyScope::View);
    registry.remove(&removed).unwrap();
    let mut other = SCDependencies::new();
    let foreign = other.register(SCDependencyScope::Device);
    let captured = registry.snapshot(std::slice::from_ref(&valid)).unwrap();
    for (bad, reason) in [
        (foreign, SCDependencyError::ForeignRegistry),
        (removed, SCDependencyError::UnknownDependency),
    ] {
        assert_eq!(
            registry.advance(&[valid.clone(), bad.clone()]).unwrap_err(),
            reason
        );
        assert_eq!(registry.validate(&captured), Ok(()));
        assert_eq!(
            registry.snapshot(std::slice::from_ref(&bad)).unwrap_err(),
            reason
        );
        assert_eq!(registry.remove(&bad), Err(reason));
        assert_eq!(registry.validate(&captured), Ok(()));
    }
    let empty = registry.snapshot(&[]).unwrap();
    assert!(empty.is_empty());
    assert_eq!(registry.validate(&empty), Ok(()));
    assert_eq!(
        other.validate(&empty),
        Err(SCDependencyError::ForeignRegistry)
    );
}

#[test]
fn removal_and_same_scope_recreation_cannot_restore_old_authority() {
    let mut registry = SCDependencies::new();
    let old = registry.register(SCDependencyScope::Instance);
    let snapshot = registry.snapshot(std::slice::from_ref(&old)).unwrap();
    registry.remove(&old).unwrap();
    let replacement = registry.register(SCDependencyScope::Instance);
    assert_ne!(old, replacement);
    assert_eq!(
        registry.validate(&snapshot),
        Err(SCDependencyError::UnknownDependency)
    );
    assert_eq!(
        registry.remove(&old),
        Err(SCDependencyError::UnknownDependency)
    );
    let current = registry.snapshot(&[replacement]).unwrap();
    assert_eq!(registry.validate(&current), Ok(()));
}
