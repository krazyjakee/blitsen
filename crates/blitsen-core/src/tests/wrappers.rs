use super::*;

fn get_or_create<F>(
    table: &WrapperTable<u64, Weak<MockObject>>,
    engine: &mut MockEngine,
    node: u64,
    create: F,
) -> Result<Rc<MockObject>, JsError>
where
    F: FnOnce(&mut MockEngine, MockFinalizer) -> Result<Rc<MockObject>, JsError>,
{
    table.get_or_create_with(
        engine,
        node,
        |engine, reference| engine.upgrade(reference),
        create,
        |engine, wrapper| engine.downgrade(wrapper),
    )
}

#[test]
fn repeated_lookups_preserve_strict_object_identity() {
    let table = WrapperTable::new();
    let mut engine = MockEngine;
    let node = 4_u64;
    let first = get_or_create(&table, &mut engine, node, |_, finalizer| {
        Ok(wrapper(ExternalId(node), finalizer))
    })
    .unwrap();
    let second = get_or_create(&table, &mut engine, node, |_, _| {
        panic!("created a duplicate wrapper")
    })
    .unwrap();

    assert!(Rc::ptr_eq(&first, &second));
    let mut weak_map = HashMap::new();
    weak_map.insert(Rc::as_ptr(&first), "value");
    assert_eq!(weak_map.get(&Rc::as_ptr(&second)), Some(&"value"));
}

#[test]
fn finalizers_remove_only_the_wrapper_generation_they_own() {
    let table = WrapperTable::new();
    let mut engine = MockEngine;
    let node = 1_u64;
    let live_wrapper = get_or_create(&table, &mut engine, node, |_, finalizer| {
        Ok(wrapper(ExternalId(node), finalizer))
    })
    .unwrap();
    assert_eq!(table.len(), 1);
    drop(live_wrapper);
    assert!(table.is_empty());

    let replacement = get_or_create(&table, &mut engine, node, |_, finalizer| {
        Ok(wrapper(ExternalId(node), finalizer))
    })
    .unwrap();
    assert_eq!(table.len(), 1);
    drop(replacement);
    assert!(table.is_empty());
}

#[test]
fn churning_one_hundred_thousand_nodes_does_not_grow_the_table() {
    let table = WrapperTable::new();
    let mut engine = MockEngine;
    for node in 0..100_000_u64 {
        let wrapper = get_or_create(&table, &mut engine, node, |_, finalizer| {
            Ok(wrapper(ExternalId(node), finalizer))
        })
        .unwrap();
        drop(wrapper);
    }
    assert!(table.is_empty());
}
