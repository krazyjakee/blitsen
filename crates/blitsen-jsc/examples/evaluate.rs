//! Loads JavaScriptCore dynamically and evaluates a numeric expression.

use std::{env, process::ExitCode};

use blitsen_jsc::JavaScriptCore;

fn main() -> ExitCode {
    let source = env::args().nth(1).unwrap_or_else(|| "6 * 7".to_owned());
    match JavaScriptCore::load().and_then(|mut runtime| runtime.evaluate_number(&source)) {
        Ok(number) => {
            println!("jsc={number}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
