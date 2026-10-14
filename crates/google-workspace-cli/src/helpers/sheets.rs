// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::Helper;
use crate::auth;
use crate::error::GwsError;
use crate::executor;
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

pub struct SheetsHelper;

impl Helper for SheetsHelper {
    fn inject_commands(
        &self,
        mut cmd: Command,
        _doc: &crate::discovery::RestDescription,
    ) -> Command {
        cmd = cmd.subcommand(
            Command::new("+append")
                .about("[Helper] Append a row to a spreadsheet")
                .arg(
                    Arg::new("spreadsheet")
                        .long("spreadsheet")
                        .help("Spreadsheet ID")
                        .required(true)
                        .value_name("ID"),
                )
                .arg(
                    Arg::new("values")
                        .long("values")
                        .help("Comma-separated values (simple strings)")
                        .value_name("VALUES")
                        .conflicts_with("json-values"),
                )
                .arg(
                    Arg::new("json-values")
                        .long("json-values")
                        .help("JSON array of rows, e.g. '[[\"a\",100,true],[\"c\",200,false]]'")
                        .value_name("JSON"),
                )
                .arg(
                    Arg::new("raw")
                        .long("raw")
                        .help("Store values as given (valueInputOption RAW) instead of parsing them as if typed (USER_ENTERED), which turns '00123' into 123 and '=A1' into a formula")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("range")
                        .long("range")
                        .help("Target range in A1 notation (e.g. 'Sheet2!A1'). Defaults to 'A1' (first sheet)")
                        .value_name("RANGE"),
                )
                .after_help(
                    r#"EXAMPLES:
  gws sheets +append --spreadsheet ID --values 'Alice,100,true'
  gws sheets +append --spreadsheet ID --json-values '[["a","b"],["c","d"]]'
  gws sheets +append --spreadsheet ID --json-values '[["Alice",100,true]]'
  gws sheets +append --spreadsheet ID --range "Sheet2!A1" --values 'Alice,100'
  gws sheets +append --spreadsheet ID --values '00123,=not a formula' --raw

TIPS:
  Use --values for simple single-row appends.
  Use --json-values for bulk multi-row inserts.
  Use --range to target a specific sheet tab (default: A1, i.e. first sheet)."#,
                ),
        );

        cmd = cmd.subcommand(
            Command::new("+read")
                .about("[Helper] Read values from a spreadsheet")
                .arg(
                    Arg::new("spreadsheet")
                        .long("spreadsheet")
                        .help("Spreadsheet ID")
                        .required(true)
                        .value_name("ID"),
                )
                .arg(
                    Arg::new("range")
                        .long("range")
                        .help("Range to read (e.g. 'Sheet1!A1:B2')")
                        .required(true)
                        .value_name("RANGE"),
                )
                .after_help(
                    "\
EXAMPLES:
  gws sheets +read --spreadsheet ID --range \"Sheet1!A1:D10\"
  gws sheets +read --spreadsheet ID --range Sheet1

TIPS:
  Read-only — never modifies the spreadsheet.
  For advanced options, use the raw values.get API.",
                ),
        );

        cmd
    }

    fn handle<'a>(
        &'a self,
        doc: &'a crate::discovery::RestDescription,
        matches: &'a ArgMatches,
        _sanitize_config: &'a crate::helpers::modelarmor::SanitizeConfig,
    ) -> Pin<Box<dyn Future<Output = Result<bool, GwsError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(matches) = matches.subcommand_matches("+append") {
                let config = parse_append_args(matches)?;
                let (params_str, body_str, scopes) = build_append_request(&config, doc)?;

                let scope_strs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
                let (token, auth_method) = match auth::get_token(&scope_strs).await {
                    Ok(t) => (Some(t), executor::AuthMethod::OAuth),
                    Err(_) if matches.get_flag("dry-run") => (None, executor::AuthMethod::None),
                    Err(e) => return Err(GwsError::Auth(format!("Sheets auth failed: {e}"))),
                };

                let spreadsheets_res = doc.resources.get("spreadsheets").ok_or_else(|| {
                    GwsError::Discovery("Resource 'spreadsheets' not found".to_string())
                })?;
                let values_res = spreadsheets_res.resources.get("values").ok_or_else(|| {
                    GwsError::Discovery("Resource 'spreadsheets.values' not found".to_string())
                })?;
                let append_method = values_res.methods.get("append").ok_or_else(|| {
                    GwsError::Discovery("Method 'spreadsheets.values.append' not found".to_string())
                })?;

                let pagination = executor::PaginationConfig {
                    page_all: false,
                    page_limit: 10,
                    page_delay_ms: 100,
                };

                executor::execute_method(
                    doc,
                    append_method,
                    Some(&params_str),
                    Some(&body_str),
                    token.as_deref(),
                    auth_method,
                    None,
                    None,
                    matches.get_flag("dry-run"),
                    &pagination,
                    None,
                    &crate::helpers::modelarmor::SanitizeMode::Warn,
                    &super::output_format(matches),
                    false,
                )
                .await?;

                return Ok(true);
            }

            if let Some(matches) = matches.subcommand_matches("+read") {
                let config = parse_read_args(matches);
                let (params_str, scopes) = build_read_request(&config, doc)?;

                // Re-find method
                let spreadsheets_res = doc.resources.get("spreadsheets").ok_or_else(|| {
                    GwsError::Discovery("Resource 'spreadsheets' not found".to_string())
                })?;
                let values_res = spreadsheets_res.resources.get("values").ok_or_else(|| {
                    GwsError::Discovery("Resource 'spreadsheets.values' not found".to_string())
                })?;
                let get_method = values_res.methods.get("get").ok_or_else(|| {
                    GwsError::Discovery("Method 'spreadsheets.values.get' not found".to_string())
                })?;

                let scope_strs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
                let (token, auth_method) = match auth::get_token(&scope_strs).await {
                    Ok(t) => (Some(t), executor::AuthMethod::OAuth),
                    Err(_) if matches.get_flag("dry-run") => (None, executor::AuthMethod::None),
                    Err(e) => return Err(GwsError::Auth(format!("Sheets auth failed: {e}"))),
                };

                executor::execute_method(
                    doc,
                    get_method,
                    Some(&params_str),
                    None,
                    token.as_deref(),
                    auth_method,
                    None,
                    None,
                    matches.get_flag("dry-run"),
                    &executor::PaginationConfig::default(),
                    None,
                    &crate::helpers::modelarmor::SanitizeMode::Warn,
                    &super::output_format(matches),
                    false,
                )
                .await?;

                return Ok(true);
            }

            Ok(false)
        })
    }
}

fn build_append_request(
    config: &AppendConfig,
    doc: &crate::discovery::RestDescription,
) -> Result<(String, String, Vec<String>), GwsError> {
    let spreadsheets_res = doc
        .resources
        .get("spreadsheets")
        .ok_or_else(|| GwsError::Discovery("Resource 'spreadsheets' not found".to_string()))?;
    let values_res = spreadsheets_res.resources.get("values").ok_or_else(|| {
        GwsError::Discovery("Resource 'spreadsheets.values' not found".to_string())
    })?;
    let append_method = values_res.methods.get("append").ok_or_else(|| {
        GwsError::Discovery("Method 'spreadsheets.values.append' not found".to_string())
    })?;

    let params = json!({
        "spreadsheetId": config.spreadsheet_id,
        "range": config.range,
        "valueInputOption": config.value_input_option
    });

    let body = json!({
        "values": config.values
    });

    // Map `&String` scope URLs to owned `String`s for the return value
    let scopes: Vec<String> = append_method.scopes.iter().map(|s| s.to_string()).collect();

    Ok((params.to_string(), body.to_string(), scopes))
}

fn build_read_request(
    config: &ReadConfig,
    doc: &crate::discovery::RestDescription,
) -> Result<(String, Vec<String>), GwsError> {
    // ... resource lookup omitted for brevity ...
    let spreadsheets_res = doc
        .resources
        .get("spreadsheets")
        .ok_or_else(|| GwsError::Discovery("Resource 'spreadsheets' not found".to_string()))?;
    let values_res = spreadsheets_res.resources.get("values").ok_or_else(|| {
        GwsError::Discovery("Resource 'spreadsheets.values' not found".to_string())
    })?;
    let get_method = values_res.methods.get("get").ok_or_else(|| {
        GwsError::Discovery("Method 'spreadsheets.values.get' not found".to_string())
    })?;

    let params = json!({
        "spreadsheetId": config.spreadsheet_id,
        "range": config.range
    });

    let scopes: Vec<String> = get_method.scopes.iter().map(|s| s.to_string()).collect();

    Ok((params.to_string(), scopes))
}

/// Configuration for appending values to a spreadsheet.
///
/// Holds the parsed arguments for the `+append` subcommand.
pub struct AppendConfig {
    /// The ID of the spreadsheet to append to.
    pub spreadsheet_id: String,
    /// Target range in A1 notation (e.g. "Sheet2!A1"). Defaults to "A1".
    pub range: String,
    /// The rows to append, where each inner Vec represents one row of
    /// JSON scalars (string, number, boolean or null).
    pub values: Vec<Vec<Value>>,
    /// `USER_ENTERED` (default) or `RAW` (`--raw`).
    pub value_input_option: &'static str,
}

/// Parses arguments for the `+append` command.
///
/// Supports both `--values` (single row) and `--json-values` (single or multi-row).
/// Invalid JSON or an empty row set is an error.
pub fn parse_append_args(matches: &ArgMatches) -> Result<AppendConfig, GwsError> {
    let values = if let Some(json_str) = matches.get_one::<String>("json-values") {
        parse_json_rows(json_str)?
    } else if let Some(values_str) = matches.get_one::<String>("values") {
        if values_str.is_empty() {
            return Err(GwsError::Validation(
                "--values must not be empty".to_string(),
            ));
        }
        vec![values_str
            .split(',')
            .map(|s| Value::String(s.to_string()))
            .collect()]
    } else {
        return Err(GwsError::Validation(
            "One of --values or --json-values is required".to_string(),
        ));
    };

    let range = matches
        .get_one::<String>("range")
        .cloned()
        .unwrap_or_else(|| "A1".to_string());

    let value_input_option = if matches.get_flag("raw") {
        "RAW"
    } else {
        "USER_ENTERED"
    };

    Ok(AppendConfig {
        spreadsheet_id: matches.get_one::<String>("spreadsheet").unwrap().clone(),
        range,
        values,
        value_input_option,
    })
}

/// Parses `--json-values`: an array of rows, or a flat array as a single row.
/// Cells may be strings, numbers, booleans or null.
fn parse_json_rows(json_str: &str) -> Result<Vec<Vec<Value>>, GwsError> {
    let invalid = |detail: &str| {
        GwsError::Validation(format!(
            "--json-values {detail}; expected an array of rows, e.g. '[[\"a\",1,true]]'"
        ))
    };
    let parsed: Value =
        serde_json::from_str(json_str).map_err(|e| invalid(&format!("is not valid JSON ({e})")))?;
    let Value::Array(items) = parsed else {
        return Err(invalid("is not an array"));
    };

    let rows: Vec<Vec<Value>> = if items.iter().all(Value::is_array) {
        items
            .into_iter()
            .filter_map(|row| match row {
                Value::Array(cells) => Some(cells),
                _ => None,
            })
            .collect()
    } else if items.iter().any(Value::is_array) {
        return Err(invalid("mixes rows and single values"));
    } else {
        vec![items]
    };

    if rows.iter().all(Vec::is_empty) {
        return Err(invalid("contains no values"));
    }
    if rows.iter().flatten().any(|c| c.is_array() || c.is_object()) {
        return Err(invalid(
            "has a cell that is not a string, number, boolean or null",
        ));
    }
    Ok(rows)
}

/// Configuration for reading values from a spreadsheet.
pub struct ReadConfig {
    pub spreadsheet_id: String,
    /// A1 notation range (e.g. "Sheet1!A1:B2").
    pub range: String,
}

pub fn parse_read_args(matches: &ArgMatches) -> ReadConfig {
    ReadConfig {
        spreadsheet_id: matches.get_one::<String>("spreadsheet").unwrap().clone(),
        range: matches.get_one::<String>("range").unwrap().clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{RestDescription, RestMethod, RestResource};
    use std::collections::HashMap;

    fn make_mock_doc() -> RestDescription {
        let mut methods = HashMap::new();
        methods.insert(
            "append".to_string(),
            RestMethod {
                scopes: vec!["https://scope".to_string()],
                ..Default::default()
            },
        );
        methods.insert(
            "get".to_string(),
            RestMethod {
                scopes: vec!["https://scope".to_string()],
                ..Default::default()
            },
        );

        let mut values_res = RestResource::default();
        values_res.methods = methods;

        let mut spreadsheets_res = RestResource::default();
        spreadsheets_res
            .resources
            .insert("values".to_string(), values_res);

        let mut resources = HashMap::new();
        resources.insert("spreadsheets".to_string(), spreadsheets_res);

        RestDescription {
            resources,
            ..Default::default()
        }
    }

    fn make_matches_append(args: &[&str]) -> ArgMatches {
        let cmd = Command::new("test")
            .arg(Arg::new("spreadsheet").long("spreadsheet"))
            .arg(Arg::new("values").long("values"))
            .arg(Arg::new("json-values").long("json-values"))
            .arg(Arg::new("range").long("range"))
            .arg(Arg::new("raw").long("raw").action(ArgAction::SetTrue));
        cmd.try_get_matches_from(args).unwrap()
    }

    fn make_matches_read(args: &[&str]) -> ArgMatches {
        let cmd = Command::new("test")
            .arg(Arg::new("spreadsheet").long("spreadsheet"))
            .arg(Arg::new("range").long("range"));
        cmd.try_get_matches_from(args).unwrap()
    }

    #[test]
    fn test_build_append_request() {
        let doc = make_mock_doc();
        let config = AppendConfig {
            spreadsheet_id: "123".to_string(),
            range: "A1".to_string(),
            values: vec![vec![json!("a"), json!("b"), json!("c")]],
            value_input_option: "USER_ENTERED",
        };
        let (params, body, scopes) = build_append_request(&config, &doc).unwrap();

        assert!(params.contains("123"));
        assert!(params.contains("USER_ENTERED"));
        assert!(params.contains("A1"));
        assert!(body.contains("a"));
        assert!(body.contains("b"));
        assert_eq!(scopes[0], "https://scope");
    }

    #[test]
    fn test_build_append_request_with_range() {
        let doc = make_mock_doc();
        let config = AppendConfig {
            spreadsheet_id: "123".to_string(),
            range: "Sheet2!A1".to_string(),
            values: vec![vec![json!("x")]],
            value_input_option: "USER_ENTERED",
        };
        let (params, _body, _scopes) = build_append_request(&config, &doc).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&params).unwrap();
        assert_eq!(parsed["range"], "Sheet2!A1");
    }

    #[test]
    fn test_build_read_request() {
        let doc = make_mock_doc();
        let config = ReadConfig {
            spreadsheet_id: "123".to_string(),
            range: "A1:B2".to_string(),
        };
        let (params, scopes) = build_read_request(&config, &doc).unwrap();

        assert!(params.contains("123"));
        assert!(params.contains("A1:B2"));
        assert_eq!(scopes[0], "https://scope");
    }

    #[test]
    fn test_parse_append_args_values() {
        let matches = make_matches_append(&["test", "--spreadsheet", "123", "--values", "a,b,c"]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(config.spreadsheet_id, "123");
        assert_eq!(config.range, "A1");
        assert_eq!(
            config.values,
            vec![vec![json!("a"), json!("b"), json!("c")]]
        );
        assert_eq!(config.value_input_option, "USER_ENTERED");
    }

    #[test]
    fn test_parse_append_args_with_range() {
        let matches = make_matches_append(&[
            "test",
            "--spreadsheet",
            "123",
            "--range",
            "Sheet2!A1",
            "--values",
            "a,b",
        ]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(config.range, "Sheet2!A1");
    }

    #[test]
    fn test_parse_append_args_default_range() {
        let matches = make_matches_append(&["test", "--spreadsheet", "123", "--values", "a"]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(config.range, "A1");
    }

    #[test]
    fn test_parse_append_args_json_single_row() {
        let matches = make_matches_append(&[
            "test",
            "--spreadsheet",
            "123",
            "--json-values",
            r#"["a","b","c"]"#,
        ]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(
            config.values,
            vec![vec![json!("a"), json!("b"), json!("c")]]
        );
    }

    #[test]
    fn test_parse_append_args_json_multi_row() {
        let matches = make_matches_append(&[
            "test",
            "--spreadsheet",
            "123",
            "--json-values",
            r#"[["Alice","100"],["Bob","200"]]"#,
        ]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(
            config.values,
            vec![
                vec![json!("Alice"), json!("100")],
                vec![json!("Bob"), json!("200")]
            ]
        );
    }

    #[test]
    fn test_parse_append_args_json_mixed_types() {
        let matches = make_matches_append(&[
            "test",
            "--spreadsheet",
            "123",
            "--json-values",
            r#"[["Alice",100,true,null]]"#,
        ]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(
            config.values,
            vec![vec![json!("Alice"), json!(100), json!(true), json!(null)]]
        );

        let matches = make_matches_append(&[
            "test",
            "--spreadsheet",
            "123",
            "--json-values",
            r#"["Bob",2.5]"#,
        ]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(config.values, vec![vec![json!("Bob"), json!(2.5)]]);
    }

    #[test]
    fn test_parse_append_args_rejects_bad_or_empty_input() {
        for bad in [
            "not json",
            "[]",
            "[[]]",
            r#"{"a":1}"#,
            r#"[["a"],"b"]"#,
            r#"[[{"a":1}]]"#,
        ] {
            let matches =
                make_matches_append(&["test", "--spreadsheet", "123", "--json-values", bad]);
            assert!(parse_append_args(&matches).is_err(), "accepted {bad}");
        }

        let matches = make_matches_append(&["test", "--spreadsheet", "123", "--values", ""]);
        assert!(parse_append_args(&matches).is_err());

        let matches = make_matches_append(&["test", "--spreadsheet", "123"]);
        assert!(parse_append_args(&matches).is_err());
    }

    #[test]
    fn test_parse_append_args_raw() {
        let matches =
            make_matches_append(&["test", "--spreadsheet", "123", "--values", "00123", "--raw"]);
        let config = parse_append_args(&matches).unwrap();
        assert_eq!(config.value_input_option, "RAW");
        let (params, _, _) = build_append_request(&config, &make_mock_doc()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&params).unwrap();
        assert_eq!(parsed["valueInputOption"], "RAW");
    }

    #[test]
    fn test_build_append_request_multi_row() {
        let doc = make_mock_doc();
        let config = AppendConfig {
            spreadsheet_id: "123".to_string(),
            range: "A1".to_string(),
            values: vec![
                vec![json!("Alice"), json!("100")],
                vec![json!("Bob"), json!("200")],
            ],
            value_input_option: "USER_ENTERED",
        };
        let (_params, body, _scopes) = build_append_request(&config, &doc).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let values = parsed["values"].as_array().unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], json!(["Alice", "100"]));
        assert_eq!(values[1], json!(["Bob", "200"]));
    }

    #[test]
    fn test_parse_read_args() {
        let matches = make_matches_read(&["test", "--spreadsheet", "123", "--range", "A1:B2"]);
        let config = parse_read_args(&matches);
        assert_eq!(config.spreadsheet_id, "123");
        assert_eq!(config.range, "A1:B2");
    }

    #[test]
    fn test_inject_commands() {
        let helper = SheetsHelper;
        let cmd = Command::new("test");
        let doc = crate::discovery::RestDescription::default();

        let cmd = helper.inject_commands(cmd, &doc);
        let subcommands: Vec<_> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        assert!(subcommands.contains(&"+append"));
        assert!(subcommands.contains(&"+read"));
    }
}
