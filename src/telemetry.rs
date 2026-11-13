use serde_json::{Value, json};
use std::collections::BTreeMap;
pub fn resource_usage(path: &std::path::Path) -> Option<Value> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).ok()?;
    let events: Vec<Value> = std::io::BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|e| e["type"] == "turn.completed")
        .map(|e| e["usage"].clone())
        .collect();
    if events.is_empty() {
        return None;
    }
    let fields: BTreeMap<_, _> = ["input_tokens", "cached_input_tokens", "cache_write_input_tokens", "output_tokens", "reasoning_output_tokens"]
        .into_iter()
        .map(|key| {
            let sum = events.iter().try_fold(0u64, |sum, e| sum.checked_add(e[key].as_u64()?));
            (key, json!({"value":sum,"measuredTurns":events.iter().filter(|e| e[key].is_u64()).count(),"turns":events.len()}))
        })
        .collect();
    Some(json!({"source":"events.jsonl:turn.completed","fields":fields}))
}
pub fn total(components: &Value) -> Option<u64> {
    let value = |key: &str| components["fields"][key]["value"].as_u64();
    value("input_tokens")?.checked_add(value("output_tokens")?)
}
pub fn usage_reset(log: &str) -> Option<Option<u64>> {
    let line = log.lines().rev().find(|line| line.contains("usage limit"))?;
    let text = line.split("try again at ").nth(1).map(|t| t.trim().trim_end_matches('.'));
    Some(text.and_then(parse_local))
}
fn parse_local(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split([' ', ',']).filter(|p| !p.is_empty()).collect();
    let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let first = parts.first()?.to_lowercase();
    let month = months.iter().position(|m| first.starts_with(m))? as u16 + 1;
    let day: u16 = parts.get(1)?.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse().ok()?;
    let year: u16 = parts.get(2)?.parse().ok()?;
    let (hour, minute) = parts.get(3)?.split_once(':')?;
    let (mut hour, minute): (u16, u16) = (hour.parse().ok()?, minute.parse().ok()?);
    match parts.get(4).map(|p| p.to_ascii_uppercase()) {
        Some(p) if p == "PM" && hour < 12 => hour += 12,
        Some(p) if p == "AM" && hour == 12 => hour = 0,
        _ => {}
    }
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
    let local = SYSTEMTIME { wYear: year, wMonth: month, wDayOfWeek: 0, wDay: day, wHour: hour, wMinute: minute, wSecond: 0, wMilliseconds: 0 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut file = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    if unsafe { TzSpecificLocalTimeToSystemTime(std::ptr::null(), &local, &mut utc) } == 0 || unsafe { SystemTimeToFileTime(&utc, &mut file) } == 0 {
        return None;
    }
    let ticks = (u64::from(file.dwHighDateTime) << 32) | u64::from(file.dwLowDateTime);
    (ticks / 10_000).checked_sub(11_644_473_600_000)
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
