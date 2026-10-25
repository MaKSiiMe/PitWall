//! Anonymizes a recording before it goes into the repository:
//! `cargo run -p pitwall-gbx --features mock --example anonymize -- <in.jsonl> <out.jsonl>`.
//! Logins and account ids become fake ones, nicknames `Player N`, chat text `…`. Read the result
//! before committing it: anything else typed by players stays.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: anonymize <in.jsonl> <out.jsonl>");
        return ExitCode::FAILURE;
    };
    let text = match std::fs::read_to_string(input) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("cannot read {input}: {err}");
            return ExitCode::FAILURE;
        }
    };
    match pitwall_gbx::replay::anonymize(&text) {
        Ok(out) => match std::fs::write(output, out) {
            Ok(()) => {
                println!("anonymized → {output}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("cannot write {output}: {err}");
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            eprintln!("{input}: {err}");
            ExitCode::FAILURE
        }
    }
}
