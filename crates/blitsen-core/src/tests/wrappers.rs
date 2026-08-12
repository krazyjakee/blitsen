use super::*;

#[test]
fn repeated_lookups_preserve_strict_object_identity() {
    let table = WrapperTable::new();
    let mut engine = MockEngine;
    let node = NodeId::new(4, 2);
    let first = table
        .get_or_create(&mut engine, node, |_, finalizer| {
            Ok(wrapper(ExternalId(node.to_u64()), finalizer))
        })
        .unwrap();
    let second = table
        .get_or_create(&mut engine, node, |_, _| {
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
    let node = NodeId::new(1, 0);
    let live_wrapper = table
        .get_or_create(&mut engine, node, |_, finalizer| {
            Ok(wrapper(ExternalId(node.to_u64()), finalizer))
        })
        .unwrap();
    assert_eq!(table.len(), 1);
    drop(live_wrapper);
    assert!(table.is_empty());

    let replacement = table
        .get_or_create(&mut engine, node, |_, finalizer| {
            Ok(wrapper(ExternalId(node.to_u64()), finalizer))
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
    for slot in 0..100_000 {
        let node = NodeId::new(slot, 0);
        let wrapper = table
            .get_or_create(&mut engine, node, |_, finalizer| {
                Ok(wrapper(ExternalId(node.to_u64()), finalizer))
            })
            .unwrap();
        drop(wrapper);
    }
    assert!(table.is_empty());
}
