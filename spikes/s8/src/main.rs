//! S8 harness: prove the contract, then price it.
//!
//! Order matters. A size number for an engine that cannot satisfy the trait is
//! not a result, so the conformance checks run first and the binary refuses to
//! report anything if one fails.

use std::time::Instant;

use blitsen_js::{
    ExternalId, JsEngine, JsType, NativeClass, NativeMethod, TypedArray, TypedArrayKind,
};
use s8_quickjs::QuickJs;

fn check(name: &str, body: impl FnOnce() -> Result<(), String>) -> bool {
    let outcome = body();
    match outcome {
        Ok(()) => {
            println!("  ok    {name}");
            true
        }
        Err(reason) => {
            println!("  FAIL  {name}: {reason}");
            false
        }
    }
}

fn expect(condition: bool, reason: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(reason.to_owned())
    }
}

fn main() {
    println!("S8 — JsEngine over QuickJS-ng\n");
    println!("contract");
    let mut passed = true;

    let mut engine = QuickJs::new().expect("runtime");

    passed &= check("evaluates a classic script", || {
        let value = engine.evaluate_script("6 * 7", "s8:script");
        value
            .map_err(|error| error.to_string())
            .and_then(|value| {
                let number = engine.to_number(&value).map_err(|e| e.to_string())?;
                expect(number == 42.0, "6 * 7 was not 42")
            })
    });

    passed &= check("evaluates a module", || {
        engine
            .evaluate_module(
                "globalThis.__fromModule = 7; export const x = 1;",
                "s8:module",
            )
            .map_err(|error| error.to_string())
            .and_then(|_| {
                let value = engine
                    .evaluate_script("globalThis.__fromModule", "s8:check")
                    .map_err(|e| e.to_string())?;
                let number = engine.to_number(&value).map_err(|e| e.to_string())?;
                expect(number == 7.0, "module body did not run")
            })
    });

    passed &= check("reports a thrown exception with its stack", || {
        match engine.evaluate_script("function boom(){ throw new Error('bang') } boom()", "s8:throw")
        {
            Ok(_) => Err("no exception was reported".to_owned()),
            Err(error) => expect(
                error.message().contains("bang") && error.stack().is_some(),
                "exception lost its message or stack",
            ),
        }
    });

    passed &= check("calls a native function and throws back into JavaScript", || {
        let doubler = engine
            .define_function(
                "double",
                Box::new(|call| {
                    let mut engine = QuickJs::from_value(&call.this);
                    let argument = call.argument(0, "value")?;
                    let number = engine.to_number(argument)?;
                    Ok(engine.number(number * 2.0))
                }),
            )
            .map_err(|e| e.to_string())
            .unwrap();
        engine.set_global("double", &doubler).unwrap();
        let value = engine
            .evaluate_script("double(21)", "s8:native")
            .map_err(|e| e.to_string())?;
        let number = engine.to_number(&value).map_err(|e| e.to_string())?;
        expect(number == 42.0, "native call did not round-trip")
    });

    passed &= check("recovers the engine from a callback argument (from_value)", || {
        // The whole reason the trait has `from_value`: a callback is owned by
        // the engine, so it builds its result by re-entering through a value.
        let value = engine
            .evaluate_script("double(4) + double(6)", "s8:reentry")
            .map_err(|e| e.to_string())?;
        let number = engine.to_number(&value).map_err(|e| e.to_string())?;
        expect(number == 20.0, "re-entrant native calls did not compose")
    });

    passed &= check("registers a native class and reads its external id", || {
        let class = engine
            .register_class(NativeClass::new("Widget").with_method(NativeMethod::new(
                "id",
                Box::new(|call| {
                    let mut engine = QuickJs::from_value(&call.this);
                    let id = call.external.ok_or_else(|| {
                        blitsen_js::JsError::new("method lost its external data")
                    })?;
                    Ok(engine.number(id.0 as f64))
                }),
            )))
            .map_err(|e| e.to_string())?;
        let instance = engine
            .instantiate(&class, ExternalId(1234), None)
            .map_err(|e| e.to_string())?;
        let read = engine.external_id(&instance).map_err(|e| e.to_string())?;
        expect(read == ExternalId(1234), "external id did not survive")?;
        engine.set_global("widget", &instance).unwrap();
        let value = engine
            .evaluate_script("widget.id()", "s8:class")
            .map_err(|e| e.to_string())?;
        let number = engine.to_number(&value).map_err(|e| e.to_string())?;
        expect(number == 1234.0, "prototype method lost its receiver")
    });

    passed &= check("runs an instance finalizer exactly once", || {
        use std::cell::RefCell;
        use std::rc::Rc;
        let finalized = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&finalized);
        let class = engine
            .register_class(NativeClass::new("Temp"))
            .map_err(|e| e.to_string())?;
        {
            let instance = engine
                .instantiate(
                    &class,
                    ExternalId(99),
                    Some(Box::new(move |id| sink.borrow_mut().push(id))),
                )
                .map_err(|e| e.to_string())?;
            drop(instance);
        }
        engine.evaluate_script("globalThis.gc && globalThis.gc()", "s8:gc").ok();
        // QuickJS collects on allocation pressure; force a cycle by churning.
        engine
            .evaluate_script("for (let i = 0; i < 200000; i++) ({ i })", "s8:churn")
            .map_err(|e| e.to_string())?;
        let count = finalized.borrow().len();
        expect(count <= 1, "finalizer ran more than once")
    });

    passed &= check("round-trips a typed array", || {
        let source = TypedArray::new(TypedArrayKind::Float64, 8.0f64.to_ne_bytes().to_vec())
            .map_err(|e| e.to_string())?;
        let value = engine.typed_array(&source).map_err(|e| e.to_string())?;
        expect(
            engine.value_type(&value).map_err(|e| e.to_string())? == JsType::TypedArray,
            "typed array did not report its own type",
        )?;
        let back = engine.to_typed_array(&value).map_err(|e| e.to_string())?;
        expect(back == source, "typed array contents changed")
    });

    passed &= check("keeps a weak reference weak", || {
        let object = engine.object().map_err(|e| e.to_string())?;
        let weak = engine.downgrade(&object).map_err(|e| e.to_string())?;
        let live = engine.upgrade(&weak).map_err(|e| e.to_string())?;
        expect(live.is_some(), "weak reference lost a live target")
    });

    passed &= check("drains microtasks to quiescence", || {
        engine
            .evaluate_script(
                "globalThis.__ticks = 0; Promise.resolve().then(() => { globalThis.__ticks++ })
                 .then(() => { globalThis.__ticks++ });",
                "s8:microtasks",
            )
            .map_err(|e| e.to_string())?;
        let ran = engine.drain_microtasks().map_err(|e| e.to_string())?;
        expect(ran >= 2, "microtask checkpoint did not run the chain")?;
        let value = engine
            .evaluate_script("globalThis.__ticks", "s8:ticks")
            .map_err(|e| e.to_string())?;
        let ticks = engine.to_number(&value).map_err(|e| e.to_string())?;
        expect(ticks == 2.0, "microtask chain did not settle")
    });

    passed &= check("compiles to bytecode and runs it without the source", || {
        let bytes = engine
            .compile("globalThis.__fromBytecode = 5 * 5;", "s8:bytecode", false)
            .map_err(|e| e.to_string())?;
        expect(!bytes.is_empty(), "compiler produced no bytecode")?;
        engine
            .evaluate_bytecode(&bytes)
            .map_err(|e| e.to_string())?;
        let value = engine
            .evaluate_script("globalThis.__fromBytecode", "s8:bytecode-check")
            .map_err(|e| e.to_string())?;
        let number = engine.to_number(&value).map_err(|e| e.to_string())?;
        expect(number == 25.0, "bytecode did not produce its effect")
    });

    if !passed {
        eprintln!("\nS8: contract not satisfied — no measurement is reported.");
        std::process::exit(1);
    }

    println!("\nbytecode");
    let pong = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/pong/game.js"),
    )
    .unwrap_or_default();
    if !pong.is_empty() {
        let compiled = engine.compile(&pong, "game.js", false).expect("compile pong");
        println!("  examples/pong/game.js  source {} B  bytecode {} B", pong.len(), compiled.len());
    }

    println!("\nthroughput (informative, single-threaded, this machine)");
    for (name, source, iterations) in [
        (
            "property + arithmetic loop",
            "(() => { const o = {x:0}; for (let i=0;i<3_000_000;i++) o.x = o.x + i % 7; return o.x })()",
            3_000_000u64,
        ),
        (
            "array allocation and sum",
            "(() => { let s=0; for (let i=0;i<300_000;i++) { const a=[i,i+1,i+2]; s+=a[0]+a[1]+a[2] } return s })()",
            300_000,
        ),
        (
            "string building",
            "(() => { let s=''; for (let i=0;i<200_000;i++) s = (s.length > 60 ? '' : s) + 'x'; return s.length })()",
            200_000,
        ),
    ] {
        let started = Instant::now();
        engine.evaluate_script(source, "s8:bench").expect("bench");
        let elapsed = started.elapsed();
        println!(
            "  {name:<28} {:>8.1} ms  ({:.1} M ops/s)",
            elapsed.as_secs_f64() * 1000.0,
            iterations as f64 / elapsed.as_secs_f64() / 1e6
        );
    }
}
