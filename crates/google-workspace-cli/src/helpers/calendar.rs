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
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::json;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct CalendarHelper;

impl Helper for CalendarHelper {
    fn inject_commands(
        &self,
        mut cmd: Command,
        _doc: &crate::discovery::RestDescription,
    ) -> Command {
        cmd = cmd.subcommand(
            Command::new("+insert")
                .about("[Helper] create a new event")
                .arg(
                    Arg::new("calendar")
                        .long("calendar")
                        .help("Calendar ID (default: primary)")
                        .default_value("primary")
                        .value_name("ID"),
                )
                .arg(
                    Arg::new("summary")
                        .long("summary")
                        .help("Event summary/title")
                        .required(true)
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("start")
                        .long("start")
                        .help("Start time (ISO 8601, e.g., 2024-01-01T10:00:00Z)")
                        .required(true)
                        .value_name("TIME"),
                )
                .arg(
                    Arg::new("end")
                        .long("end")
                        .help("End time (ISO 8601)")
                        .required(true)
                        .value_name("TIME"),
                )
                .arg(
                    Arg::new("location")
                        .long("location")
                        .help("Event location")
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("description")
                        .long("description")
                        .help("Event description/body")
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("attendee")
                        .long("attendee")
                        .help("Attendee email (can be used multiple times)")
                        .value_name("EMAIL")
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("meet")
                        .long("meet")
                        .help("Add a Google Meet video conference link")
                        .action(ArgAction::SetTrue),
                )
                .after_help("\
EXAMPLES:
  gws calendar +insert --summary 'Standup' --start '2026-06-17T09:00:00-07:00' --end '2026-06-17T09:30:00-07:00'
  gws calendar +insert --summary 'Review' --start ... --end ... --attendee alice@example.com
  gws calendar +insert --summary 'Meet' --start ... --end ... --meet

TIPS:
  Use RFC3339 format for times (e.g. 2026-06-17T09:00:00-07:00).
  The --meet flag automatically adds a Google Meet link to the event."),
        );
        cmd = cmd.subcommand(
            Command::new("+agenda")
                .about("[Helper] Show upcoming events across all calendars")
                .arg(
                    Arg::new("today")
                        .long("today")
                        .help("Show today's events")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("tomorrow")
                        .long("tomorrow")
                        .help("Show tomorrow's events")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("week")
                        .long("week")
                        .help("Show this week's events")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("days")
                        .long("days")
                        .help("Number of days ahead to show")
                        .value_name("N"),
                )
                .arg(
                    Arg::new("calendar")
                        .long("calendar")
                        .help("Filter to specific calendar name or ID")
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("timezone")
                        .long("timezone")
                        .alias("tz")
                        .help("IANA timezone override (e.g. America/Denver). Defaults to Google account timezone.")
                        .value_name("TZ"),
                )
                .after_help(
                    "\
EXAMPLES:
  gws calendar +agenda
  gws calendar +agenda --today
  gws calendar +agenda --week --format table
  gws calendar +agenda --days 3 --calendar 'Work'
  gws calendar +agenda --today --timezone America/New_York

TIPS:
  Read-only — never modifies events.
  Queries all calendars by default; use --calendar to filter.
  Uses your Google account timezone by default; override with --timezone.",
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
            if let Some(matches) = matches.subcommand_matches("+insert") {
                let (params_str, body_str, scopes) = build_insert_request(matches, doc)?;

                let scopes_str: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
                let (token, auth_method) = match auth::get_token(&scopes_str).await {
                    Ok(t) => (Some(t), executor::AuthMethod::OAuth),
                    Err(_) if matches.get_flag("dry-run") => (None, executor::AuthMethod::None),
                    Err(e) => return Err(GwsError::Auth(format!("Calendar auth failed: {e}"))),
                };

                let events_res = doc.resources.get("events").ok_or_else(|| {
                    GwsError::Discovery("Resource 'events' not found".to_string())
                })?;
                let insert_method = events_res.methods.get("insert").ok_or_else(|| {
                    GwsError::Discovery("Method 'events.insert' not found".to_string())
                })?;

                executor::execute_method(
                    doc,
                    insert_method,
                    Some(&params_str),
                    Some(&body_str),
                    token.as_deref(),
                    auth_method,
                    None,
                    None,
                    matches.get_flag("dry-run"),
                    &executor::PaginationConfig::default(),
                    None,
                    &crate::helpers::modelarmor::SanitizeMode::Warn,
                    &crate::formatter::OutputFormat::default(),
                    false,
                )
                .await?;

                return Ok(true);
            }
            if let Some(matches) = matches.subcommand_matches("+agenda") {
                handle_agenda(matches).await?;
                return Ok(true);
            }
            Ok(false)
        })
    }
}
async fn handle_agenda(matches: &ArgMatches) -> Result<(), GwsError> {
    let cal_scope = "https://www.googleapis.com/auth/calendar.readonly";
    let token = auth::get_token(&[cal_scope])
        .await
        .map_err(|e| GwsError::Auth(format!("Calendar auth failed: {e}")))?;

    let output_format = matches
        .get_one::<String>("format")
        .map(|s| crate::formatter::OutputFormat::from_str(s))
        .unwrap_or(crate::formatter::OutputFormat::Table);

    let client = crate::client::build_client()?;
    let tz_override = matches.get_one::<String>("timezone").map(|s| s.as_str());
    let tz = crate::timezone::resolve_account_timezone(&client, &token, tz_override).await?;

    // Determine time range using the account timezone so that --today and
    // --tomorrow align with the user's Google account day, not the machine.
    let days: i64 = if matches.get_flag("tomorrow") {
        1
    } else if matches.get_flag("week") {
        7
    } else {
        matches
            .get_one::<String>("days")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1)
    };
    let range = if matches.get_flag("today") {
        AgendaRange::Today
    } else if matches.get_flag("tomorrow") {
        AgendaRange::Tomorrow
    } else {
        AgendaRange::Days(days)
    };
    let (time_min_dt, time_max_dt) = agenda_time_range(tz, Utc::now(), range)?;

    let time_min = time_min_dt.to_rfc3339();
    let time_max = time_max_dt.to_rfc3339();

    // client already built above for timezone resolution
    let calendar_filter = matches.get_one::<String>("calendar");

    // 1. List all calendars
    let list_url = format!("{CALENDAR_API}/users/me/calendarList");
    let calendars = fetch_all_pages(|page_token| {
        let (client, list_url, token) = (&client, &list_url, &token);
        async move {
            let query: Vec<(&str, &str)> = page_token
                .as_deref()
                .map(|t| vec![("pageToken", t)])
                .unwrap_or_default();
            get_calendar_json(client, list_url, token, &query, "calendarList_failed").await
        }
    })
    .await?;

    // 2. For each calendar, fetch events concurrently
    use futures_util::stream::{self, StreamExt};

    // Pre-filter calendars and collect owned data to avoid lifetime issues
    struct CalInfo {
        id: String,
        summary: String,
    }
    let filtered_calendars: Vec<CalInfo> = calendars
        .iter()
        .filter_map(|cal| {
            let cal_id = cal.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let cal_summary = cal
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or(cal_id);

            // Apply calendar filter
            if let Some(filter) = calendar_filter {
                if !cal_summary.contains(filter.as_str()) && cal_id != filter.as_str() {
                    return None;
                }
            }

            Some(CalInfo {
                id: cal_id.to_string(),
                summary: cal_summary.to_string(),
            })
        })
        .collect();

    let results: Vec<(String, Result<Vec<Value>, GwsError>)> = stream::iter(filtered_calendars)
        .map(|cal| {
            let client = &client;
            let token = &token;
            let time_min = &time_min;
            let time_max = &time_max;
            async move {
                let events_url = format!(
                    "{CALENDAR_API}/calendars/{}/events",
                    crate::validate::encode_path_segment(&cal.id),
                );
                let events = fetch_all_pages(|page_token| {
                    let events_url = &events_url;
                    async move {
                        let mut query = events_list_query(time_min, time_max, tz);
                        if let Some(t) = page_token.as_deref() {
                            query.push(("pageToken", t));
                        }
                        get_calendar_json(client, events_url, token, &query, "events_list_failed")
                            .await
                    }
                })
                .await
                .map(|items| {
                    items
                        .iter()
                        .map(|event| agenda_entry(event, &cal.summary))
                        .collect()
                });
                (cal.summary, events)
            }
        })
        .buffer_unordered(5)
        .collect()
        .await;

    let (mut all_events, failures) = merge_calendar_results(results);
    for failure in &failures {
        eprintln!("Warning: {}", crate::output::sanitize_for_terminal(failure));
    }

    // 3. Sort by start time
    sort_events(&mut all_events, tz);

    let output = json!({
        "events": all_events,
        "count": all_events.len(),
        "timeMin": time_min,
        "timeMax": time_max,
    });

    println!(
        "{}",
        crate::formatter::format_value(&output, &output_format)
    );

    // The agenda above is incomplete: exit non-zero without a second JSON
    // document on stdout (the warnings already named each calendar).
    if !failures.is_empty() {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::process::exit(GwsError::EXIT_CODE_API);
    }
    Ok(())
}

const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

/// GET a Calendar API URL and parse its JSON body.
async fn get_calendar_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    query: &[(&str, &str)],
    reason: &str,
) -> Result<Value, GwsError> {
    let resp = crate::client::send_with_retry(|| client.get(url).query(query).bearer_auth(token))
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(GwsError::Api {
            code: status.as_u16(),
            message: body,
            reason: reason.to_string(),
            enable_url: None,
        });
    }

    resp.json()
        .await
        .map_err(|e| GwsError::Other(anyhow::anyhow!("Failed to parse response: {e}")))
}

/// Collect `items` from a paged list endpoint. `fetch_page` gets the page
/// token to request (`None` for the first page).
async fn fetch_all_pages<F, Fut>(mut fetch_page: F) -> Result<Vec<Value>, GwsError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<Value, GwsError>>,
{
    let mut items = Vec::new();
    let mut page_token = None;
    loop {
        let page = fetch_page(page_token.take()).await?;
        if let Some(page_items) = page.get("items").and_then(|i| i.as_array()) {
            items.extend(page_items.iter().cloned());
        }
        match page.get("nextPageToken").and_then(|t| t.as_str()) {
            Some(token) if !token.is_empty() => page_token = Some(token.to_string()),
            _ => return Ok(items),
        }
    }
}

fn agenda_entry(event: &Value, calendar: &str) -> Value {
    let time = |key: &str| {
        event
            .get(key)
            .and_then(|s| s.get("dateTime").or_else(|| s.get("date")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    json!({
        "start": time("start"),
        "end": time("end"),
        "summary": event.get("summary").and_then(|v| v.as_str()).unwrap_or("(No title)"),
        "calendar": calendar,
        "location": event.get("location").and_then(|v| v.as_str()).unwrap_or(""),
    })
}

/// Combine per-calendar results into one event list, plus a message for
/// each calendar whose events could not be fetched.
fn merge_calendar_results(
    results: Vec<(String, Result<Vec<Value>, GwsError>)>,
) -> (Vec<Value>, Vec<String>) {
    let mut events = Vec::new();
    let mut failures = Vec::new();
    for (calendar, result) in results {
        match result {
            Ok(items) => events.extend(items),
            Err(e) => failures.push(format!(
                "could not fetch events for calendar '{calendar}': {}",
                describe_error(&e)
            )),
        }
    }
    (events, failures)
}

fn describe_error(err: &GwsError) -> String {
    match err {
        GwsError::Api { code, message, .. } => {
            let detail = serde_json::from_str::<Value>(message)
                .ok()
                .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_string))
                .unwrap_or_else(|| message.clone());
            format!("HTTP {code}: {detail}")
        }
        other => other.to_string(),
    }
}

/// Parse an event start (RFC3339 `dateTime`, or all-day `date` taken as the
/// start of that day in `tz`) into an instant.
fn event_start_instant(start: &str, tz: Tz) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(start) {
        return Some(dt.with_timezone(&Utc));
    }
    let date = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d").ok()?;
    crate::timezone::start_of_day(tz, date)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Sort by start instant; offsets differ between events, so the strings
/// don't sort chronologically. Unparseable starts go last.
fn sort_events(events: &mut [Value], tz: Tz) {
    events.sort_by_cached_key(|e| {
        let instant = e
            .get("start")
            .and_then(|v| v.as_str())
            .and_then(|s| event_start_instant(s, tz));
        (instant.is_none(), instant)
    });
}

fn events_list_query<'a>(
    time_min: &'a str,
    time_max: &'a str,
    tz: Tz,
) -> Vec<(&'static str, &'a str)> {
    vec![
        ("timeMin", time_min),
        ("timeMax", time_max),
        ("singleEvents", "true"),
        ("orderBy", "startTime"),
        ("maxResults", "250"),
        // Return event times in the account zone rather than each calendar's.
        ("timeZone", tz.name()),
    ]
}

enum AgendaRange {
    Today,
    Tomorrow,
    /// From now, N days ahead.
    Days(i64),
}

fn agenda_time_range(
    tz: Tz,
    now: DateTime<Utc>,
    range: AgendaRange,
) -> Result<(DateTime<Tz>, DateTime<Tz>), GwsError> {
    let now_in_tz = now.with_timezone(&tz);
    let today = now_in_tz.date_naive();
    match range {
        AgendaRange::Today => crate::timezone::day_range(tz, today),
        AgendaRange::Tomorrow => {
            let tomorrow = today
                .succ_opt()
                .ok_or_else(|| GwsError::Validation(format!("Date out of range: {today}")))?;
            crate::timezone::day_range(tz, tomorrow)
        }
        AgendaRange::Days(days) => Ok((now_in_tz, now_in_tz + chrono::Duration::days(days))),
    }
}

fn build_insert_request(
    matches: &ArgMatches,
    doc: &crate::discovery::RestDescription,
) -> Result<(String, String, Vec<String>), GwsError> {
    let calendar_id = matches.get_one::<String>("calendar").unwrap();
    let summary = matches.get_one::<String>("summary").unwrap();
    let start = matches.get_one::<String>("start").unwrap();
    let end = matches.get_one::<String>("end").unwrap();
    let location = matches.get_one::<String>("location");
    let description = matches.get_one::<String>("description");
    let attendees_vals = matches.get_many::<String>("attendee");

    // Find method: events.insert checks
    let events_res = doc
        .resources
        .get("events")
        .ok_or_else(|| GwsError::Discovery("Resource 'events' not found".to_string()))?;
    let insert_method = events_res
        .methods
        .get("insert")
        .ok_or_else(|| GwsError::Discovery("Method 'events.insert' not found".to_string()))?;

    // Build body
    let mut body = json!({
        "summary": summary,
        "start": { "dateTime": start },
        "end": { "dateTime": end },
    });

    if let Some(loc) = location {
        body["location"] = json!(loc);
    }
    if let Some(desc) = description {
        body["description"] = json!(desc);
    }

    if let Some(atts) = attendees_vals {
        let attendees_list: Vec<_> = atts.map(|email| json!({ "email": email })).collect();
        body["attendees"] = json!(attendees_list);
    }

    let mut params = json!({
        "calendarId": calendar_id
    });

    if matches.get_flag("meet") {
        let namespace = uuid::Uuid::NAMESPACE_DNS;

        let mut attendees: Vec<_> = matches
            .get_many::<String>("attendee")
            .map(|vals| vals.cloned().collect())
            .unwrap_or_default();
        attendees.sort();

        let seed_payload = {
            let mut map = serde_json::Map::new();
            map.insert("v".to_string(), json!(1));
            map.insert("summary".to_string(), json!(summary));
            map.insert("start".to_string(), json!(start));
            map.insert("end".to_string(), json!(end));
            if let Some(loc) = location {
                map.insert("location".to_string(), json!(loc));
            }
            if let Some(desc) = description {
                map.insert("description".to_string(), json!(desc));
            }
            if !attendees.is_empty() {
                let attendees_list_for_seed: Vec<_> = attendees
                    .iter()
                    .map(|email| json!({ "email": email }))
                    .collect();
                map.insert("attendees".to_string(), json!(attendees_list_for_seed));
            }
            serde_json::Value::Object(map)
        };

        let seed_data = serde_json::to_vec(&seed_payload).map_err(|e| {
            GwsError::Other(anyhow::anyhow!(
                "Failed to serialize seed payload for idempotency key: {e}"
            ))
        })?;
        let request_id = uuid::Uuid::new_v5(&namespace, &seed_data).to_string();

        body["conferenceData"] = json!({
            "createRequest": {
                "requestId": request_id,
                "conferenceSolutionKey": { "type": "hangoutsMeet" }
            }
        });
        params["conferenceDataVersion"] = json!(1);
    }
    let body_str = body.to_string();
    let scopes: Vec<String> = insert_method.scopes.iter().map(|s| s.to_string()).collect();

    // events.insert requires 'calendarId' path parameter
    let params_str = params.to_string();

    Ok((params_str, body_str, scopes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_mock_doc() -> crate::discovery::RestDescription {
        let mut doc = crate::discovery::RestDescription::default();
        let mut events_res = crate::discovery::RestResource::default();
        let mut insert_method = crate::discovery::RestMethod::default();
        insert_method.scopes.push("https://scope".to_string());
        events_res
            .methods
            .insert("insert".to_string(), insert_method);
        doc.resources.insert("events".to_string(), events_res);
        doc
    }

    fn make_matches_insert(args: &[&str]) -> ArgMatches {
        let cmd = Command::new("test")
            .arg(
                Arg::new("calendar")
                    .long("calendar")
                    .default_value("primary"),
            )
            .arg(Arg::new("summary").long("summary").required(true))
            .arg(Arg::new("start").long("start").required(true))
            .arg(Arg::new("end").long("end").required(true))
            .arg(Arg::new("location").long("location"))
            .arg(Arg::new("description").long("description"))
            .arg(
                Arg::new("attendee")
                    .long("attendee")
                    .action(ArgAction::Append),
            )
            .arg(Arg::new("meet").long("meet").action(ArgAction::SetTrue));
        cmd.try_get_matches_from(args).unwrap()
    }

    #[test]
    fn test_build_insert_request() {
        let doc = make_mock_doc();
        let matches = make_matches_insert(&[
            "test",
            "--summary",
            "Meeting",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
        ]);
        let (params, body, scopes) = build_insert_request(&matches, &doc).unwrap();

        assert!(params.contains("primary"));
        assert!(body.contains("Meeting"));
        assert!(body.contains("2024-01-01T10:00:00Z"));
        assert_eq!(scopes[0], "https://scope");
    }

    #[test]
    fn test_build_insert_request_with_meet() {
        let doc = make_mock_doc();
        let matches = make_matches_insert(&[
            "test",
            "--summary",
            "Meeting",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
            "--meet",
        ]);
        let (params, body, _) = build_insert_request(&matches, &doc).unwrap();

        let params_json: serde_json::Value = serde_json::from_str(&params).unwrap();
        assert_eq!(params_json["conferenceDataVersion"], 1);

        let body_json: serde_json::Value = serde_json::from_str(&body).unwrap();
        let create_req = &body_json["conferenceData"]["createRequest"];
        assert_eq!(create_req["conferenceSolutionKey"]["type"], "hangoutsMeet");
        assert!(uuid::Uuid::parse_str(create_req["requestId"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn test_build_insert_request_with_meet_is_idempotent() {
        let doc = make_mock_doc();
        let args = &[
            "test",
            "--summary",
            "Idempotent Meeting",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
            "--meet",
        ];
        let matches1 = make_matches_insert(args);
        let (_, body1, _) = build_insert_request(&matches1, &doc).unwrap();

        let matches2 = make_matches_insert(args);
        let (_, body2, _) = build_insert_request(&matches2, &doc).unwrap();

        let b1: serde_json::Value = serde_json::from_str(&body1).unwrap();
        let b2: serde_json::Value = serde_json::from_str(&body2).unwrap();

        assert_eq!(
            b1["conferenceData"]["createRequest"]["requestId"],
            b2["conferenceData"]["createRequest"]["requestId"],
            "requestId should be deterministic for the same event details"
        );
    }

    #[test]
    fn test_build_insert_request_with_meet_idempotency_robust() {
        let doc = make_mock_doc();

        // Base case
        let args_base = &[
            "test",
            "--summary",
            "S",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
            "--meet",
            "--attendee",
            "a@b.com",
            "--attendee",
            "c@d.com",
        ];
        let (_, body_base, _) =
            build_insert_request(&make_matches_insert(args_base), &doc).unwrap();
        let b_base: serde_json::Value = serde_json::from_str(&body_base).unwrap();
        let id_base = b_base["conferenceData"]["createRequest"]["requestId"]
            .as_str()
            .unwrap();

        // Same but different attendee order
        let args_reordered = &[
            "test",
            "--summary",
            "S",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
            "--meet",
            "--attendee",
            "c@d.com",
            "--attendee",
            "a@b.com",
        ];
        let (_, body_reordered, _) =
            build_insert_request(&make_matches_insert(args_reordered), &doc).unwrap();
        let b_reordered: serde_json::Value = serde_json::from_str(&body_reordered).unwrap();
        let id_reordered = b_reordered["conferenceData"]["createRequest"]["requestId"]
            .as_str()
            .unwrap();

        assert_eq!(
            id_base, id_reordered,
            "Attendee order should not change requestId"
        );

        // Different summary -> different ID
        let args_diff = &[
            "test",
            "--summary",
            "Diff",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
            "--meet",
            "--attendee",
            "a@b.com",
            "--attendee",
            "c@d.com",
        ];
        let (_, body_diff, _) =
            build_insert_request(&make_matches_insert(args_diff), &doc).unwrap();
        let b_diff: serde_json::Value = serde_json::from_str(&body_diff).unwrap();
        let id_diff = b_diff["conferenceData"]["createRequest"]["requestId"]
            .as_str()
            .unwrap();

        assert_ne!(
            id_base, id_diff,
            "Different summary should produce different requestId"
        );
    }

    #[test]
    fn test_build_insert_request_with_optional_fields() {
        let doc = make_mock_doc();
        let matches = make_matches_insert(&[
            "test",
            "--summary",
            "Meeting",
            "--start",
            "2024-01-01T10:00:00Z",
            "--end",
            "2024-01-01T11:00:00Z",
            "--location",
            "Room 1",
            "--description",
            "Discuss stuff",
            "--attendee",
            "a@b.com",
            "--attendee",
            "c@d.com",
        ]);
        let (_, body, _) = build_insert_request(&matches, &doc).unwrap();

        assert!(body.contains("Room 1"));
        assert!(body.contains("Discuss stuff"));
        assert!(body.contains("a@b.com"));
        assert!(body.contains("c@d.com"));
    }

    #[test]
    fn agenda_requests_events_in_account_timezone() {
        let query = events_list_query(
            "2026-10-20T00:00:00-06:00",
            "2026-10-21T00:00:00-06:00",
            chrono_tz::America::Denver,
        );
        assert!(query.contains(&("timeZone", "America/Denver")));
        assert!(query.contains(&("timeMin", "2026-10-20T00:00:00-06:00")));
    }

    #[test]
    fn agenda_sorts_by_instant_not_string() {
        let mut events: Vec<Value> = [
            "2026-10-20T09:00:00-04:00", // 13:00Z
            "2026-10-20T08:00:00-06:00", // 14:00Z
            "2026-10-20",                // all-day, 06:00Z in Denver
            "2026-10-20T10:00:00Z",
        ]
        .iter()
        .map(|start| json!({ "start": start, "summary": start }))
        .collect();
        sort_events(&mut events, chrono_tz::America::Denver);
        let order: Vec<&str> = events
            .iter()
            .map(|e| e["start"].as_str().unwrap())
            .collect();
        assert_eq!(
            order,
            [
                "2026-10-20",
                "2026-10-20T10:00:00Z",
                "2026-10-20T09:00:00-04:00",
                "2026-10-20T08:00:00-06:00",
            ]
        );
    }

    /// Serve one canned HTTP response per connection on a local port.
    async fn serve_responses(responses: Vec<(&'static str, &'static str)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0_u8; 8192];
                let _ = stream.read(&mut buf).await.unwrap();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn calendar_request_errors_keep_http_status() {
        let base = serve_responses(vec![
            ("403 Forbidden", r#"{"error":{"message":"denied"}}"#),
            ("200 OK", "not json"),
        ])
        .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let err = get_calendar_json(&client, &base, "t", &[], "calendarList_failed")
            .await
            .unwrap_err();
        assert!(
            matches!(err, GwsError::Api { code: 403, .. }),
            "got {err:?}"
        );

        let err = get_calendar_json(&client, &base, "t", &[], "events_list_failed").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn fetch_all_pages_follows_next_page_token() {
        let pages = [
            json!({ "items": [{ "id": "a" }, { "id": "b" }], "nextPageToken": "p2" }),
            json!({ "items": [{ "id": "c" }], "nextPageToken": "p3" }),
            json!({ "items": [{ "id": "d" }] }),
        ];
        let mut requested = Vec::new();
        let items = fetch_all_pages(|token| {
            requested.push(token.clone());
            let page = match token.as_deref() {
                None => pages[0].clone(),
                Some("p2") => pages[1].clone(),
                _ => pages[2].clone(),
            };
            async move { Ok(page) }
        })
        .await
        .unwrap();
        let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a", "b", "c", "d"]);
        assert_eq!(
            requested,
            [None, Some("p2".to_string()), Some("p3".to_string())]
        );
    }

    #[tokio::test]
    async fn fetch_all_pages_fails_when_a_later_page_fails() {
        let result = fetch_all_pages(|token| async move {
            match token {
                None => Ok(json!({ "items": [{ "id": "a" }], "nextPageToken": "p2" })),
                Some(_) => Err(GwsError::Other(anyhow::anyhow!("boom"))),
            }
        })
        .await;
        assert!(result.is_err());
    }

    #[test]
    fn agenda_reports_calendars_that_failed() {
        let results = vec![
            (
                "Work".to_string(),
                Ok(vec![json!({ "summary": "Standup" })]),
            ),
            (
                "Team Offsite".to_string(),
                Err(GwsError::Api {
                    code: 404,
                    message: "Not Found".to_string(),
                    reason: "events_list_failed".to_string(),
                    enable_url: None,
                }),
            ),
        ];
        let (events, failures) = merge_calendar_results(results);
        assert_eq!(events.len(), 1);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("Team Offsite"), "{}", failures[0]);
        assert!(failures[0].contains("404"), "{}", failures[0]);
    }

    fn agenda_range_at(range: AgendaRange, now: &str) -> (String, String) {
        let now = chrono::DateTime::parse_from_rfc3339(now)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (min, max) = agenda_time_range(chrono_tz::America::Denver, now, range).unwrap();
        (min.to_rfc3339(), max.to_rfc3339())
    }

    #[test]
    fn agenda_today_covers_whole_dst_day() {
        // 25-hour day (fall back)
        assert_eq!(
            agenda_range_at(AgendaRange::Today, "2026-11-01T12:00:00-07:00"),
            (
                "2026-11-01T00:00:00-06:00".to_string(),
                "2026-11-02T00:00:00-07:00".to_string()
            )
        );
        // 23-hour day (spring forward)
        assert_eq!(
            agenda_range_at(AgendaRange::Today, "2026-03-08T12:00:00-06:00"),
            (
                "2026-03-08T00:00:00-07:00".to_string(),
                "2026-03-09T00:00:00-06:00".to_string()
            )
        );
    }

    #[test]
    fn agenda_tomorrow_next_to_dst_day() {
        assert_eq!(
            agenda_range_at(AgendaRange::Tomorrow, "2026-10-31T12:00:00-06:00"),
            (
                "2026-11-01T00:00:00-06:00".to_string(),
                "2026-11-02T00:00:00-07:00".to_string()
            )
        );
        assert_eq!(
            agenda_range_at(AgendaRange::Tomorrow, "2026-11-01T12:00:00-07:00"),
            (
                "2026-11-02T00:00:00-07:00".to_string(),
                "2026-11-03T00:00:00-07:00".to_string()
            )
        );
        assert_eq!(
            agenda_range_at(AgendaRange::Tomorrow, "2026-03-08T12:00:00-06:00"),
            (
                "2026-03-09T00:00:00-06:00".to_string(),
                "2026-03-10T00:00:00-06:00".to_string()
            )
        );
    }

    /// Verify that agenda day boundaries use a specific timezone, not UTC.
    #[test]
    fn agenda_day_boundaries_use_account_timezone() {
        use chrono::{NaiveTime, TimeZone, Utc};

        // Simulate using a known account timezone (America/Denver = UTC-7 / UTC-6 DST)
        let tz = chrono_tz::America::Denver;
        let now_in_tz = Utc::now().with_timezone(&tz);
        let today_start = now_in_tz
            .date_naive()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
        let today_start_tz = tz
            .from_local_datetime(&today_start)
            .earliest()
            .expect("midnight should resolve");

        let today_rfc = today_start_tz.to_rfc3339();
        let tomorrow_start = today_start_tz + chrono::Duration::days(1);
        let tomorrow_rfc = tomorrow_start.to_rfc3339();

        // The Denver offset should appear in the RFC3339 string (-07:00 or -06:00 for DST).
        // Crucially, it should NOT be +00:00 (UTC).
        assert!(
            today_rfc.contains("-07:00") || today_rfc.contains("-06:00"),
            "today boundary should carry Denver offset, got {today_rfc}"
        );
        assert!(
            tomorrow_rfc.contains("-07:00") || tomorrow_rfc.contains("-06:00"),
            "tomorrow boundary should carry Denver offset, got {tomorrow_rfc}"
        );
    }
}
