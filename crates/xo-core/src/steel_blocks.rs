//! Read-only Steel evaluation for Markdown fenced blocks.
//!
//! A `steel` block receives only `xo-query`: a JSON equality filter over any
//! frontmatter fields. It cannot mutate notes or access the filesystem,
//! environment, process, network, terminal, or clock.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
#[cfg(not(target_arch = "wasm32"))]
use steel::steel_vm::interrupt::InterruptHandler;
use steel::steel_vm::register_fn::RegisterFn;

use crate::Note;
use crate::domain::Frontmatter;

const MAX_BLOCK_BYTES: usize = 64 * 1024;
const MAX_RESULT_BYTES: usize = 256 * 1024;
const MAX_QUERY_RESULTS: usize = 10_000;

/// Expand fenced `steel` blocks into their returned Markdown.
#[must_use]
pub fn render_steel_blocks(body: &str, notes: &[Note]) -> String {
    let mut output = String::with_capacity(body.len());
    let mut lines = body.split_inclusive('\n');
    while let Some(line) = lines.next() {
        if line.trim_end() != "```steel" {
            output.push_str(line);
            continue;
        }
        let mut source = String::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line.trim_end() == "```" {
                closed = true;
                break;
            }
            source.push_str(line);
        }
        if !closed {
            output.push_str("```steel\n");
            output.push_str(&source);
            break;
        }
        match evaluate(&source, notes) {
            Ok(markdown) => output.push_str(&markdown),
            Err(error) => writeln!(output, "> Steel block error: {error}")
                .expect("write Steel error to String"),
        }
    }
    output
}

fn evaluate(source: &str, notes: &[Note]) -> Result<String, String> {
    if source.len() > MAX_BLOCK_BYTES {
        return Err("block exceeds the 64 KiB limit".into());
    }
    let documents = notes
        .iter()
        .take(MAX_QUERY_RESULTS)
        .map(|note| ReadOnlyNote {
            id: note.id.to_string(),
            frontmatter: note.frontmatter.clone(),
            body: note.body.clone(),
        })
        .collect::<Vec<_>>();
    let mut engine = Engine::new_sandboxed();
    engine.register_fn(
        "xo-query",
        move |filter: String| -> Result<String, String> {
            let filter: BTreeMap<String, serde_json::Value> = serde_json::from_str(&filter)
                .map_err(|error| format!("xo-query filter must be a JSON object: {error}"))?;
            let matches = documents
                .iter()
                .filter(|note| matches_filter(&note.frontmatter, &filter))
                .collect::<Vec<_>>();
            serde_json::to_string(&matches).map_err(|error| error.to_string())
        },
    );
    #[cfg(not(target_arch = "wasm32"))]
    let values = {
        let interrupt = InterruptHandler::new(&mut engine, std::time::Duration::from_millis(250));
        interrupt
            .run_with_timeout(|| engine.run(source.to_owned()))
            .map_err(|error| error.to_string())?
    };
    #[cfg(target_arch = "wasm32")]
    let values = engine
        .run(source.to_owned())
        .map_err(|error| error.to_string())?;
    let value = values
        .last()
        .ok_or_else(|| "Steel block must return a Markdown string".to_owned())?;
    let SteelVal::StringV(value) = value else {
        return Err("Steel block must return a Markdown string".into());
    };
    let output = value.to_string();
    if output.len() > MAX_RESULT_BYTES {
        return Err("block result exceeds the 256 KiB limit".into());
    }
    Ok(output)
}

#[derive(serde::Serialize)]
struct ReadOnlyNote {
    id: String,
    frontmatter: Frontmatter,
    body: String,
}

fn matches_filter(frontmatter: &Frontmatter, filter: &BTreeMap<String, serde_json::Value>) -> bool {
    filter.iter().all(|(field, expected)| {
        frontmatter.get(field).is_some_and(|actual| {
            serde_json::to_value(actual).is_ok_and(|value| value == *expected)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoteId;
    use crate::domain::FrontmatterValue;

    #[test]
    fn renders_read_only_frontmatter_queries_as_markdown() {
        let notes = vec![Note {
            id: NoteId::new("book001"),
            frontmatter: Frontmatter::from([
                ("type".into(), FrontmatterValue::String("book".into())),
                ("pages".into(), FrontmatterValue::Integer(300)),
            ]),
            body: "Read-only body".into(),
            path: "book.md".into(),
        }];
        let body = "# Stats\n```steel\n(let ([books (string->jsexpr (xo-query \"{\\\"type\\\":\\\"book\\\"}\"))])\n  (string-append \"Books: \" (number->string (length books))))\n```\n";
        assert_eq!(render_steel_blocks(body, &notes), "# Stats\nBooks: 1");
    }

    #[test]
    fn leaves_unclosed_blocks_unchanged_and_reports_invalid_code() {
        assert_eq!(render_steel_blocks("```steel\n(+ 1", &[]), "```steel\n(+ 1");
        assert!(render_steel_blocks("```steel\n(+ 1 2)\n```", &[]).contains("Steel block error"));
    }
}
