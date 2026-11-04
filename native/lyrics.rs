use regex::Regex;
use std::sync::OnceLock;
pub fn clip(raw: &str, start: f64, end: f64) -> Result<String, String> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    if raw.encode_utf16().count() > 4096 {
        return Err("歌词最多 4096 字符".into());
    }
    static STAMP: OnceLock<Regex> = OnceLock::new();
    let re = STAMP.get_or_init(|| Regex::new(r"^\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]").unwrap());
    let (mut rows, mut offset) = (Vec::<(i64, String)>::new(), 0i64);
    for line in raw.trim_start_matches('\u{feff}').lines() {
        if let Some(n) = line
            .strip_prefix("[offset:")
            .and_then(|s| s.strip_suffix(']'))
        {
            offset = n.parse::<i64>().map_err(|_| "歌词 offset 无效")?;
            if offset.abs() > 60000 {
                return Err("歌词 offset 超过 60 秒".into());
            }
            continue;
        }
        let mut rest = line;
        let mut stamps = Vec::new();
        while let Some(m) = re.captures(rest) {
            let sec = m[2].parse::<i64>().unwrap();
            if sec >= 60 {
                return Err("歌词秒数无效".into());
            }
            let fraction = m
                .get(3)
                .map(|x| format!("{:0<3}", x.as_str()))
                .unwrap_or("000".into());
            stamps.push(
                m[1].parse::<i64>().unwrap() * 60000
                    + sec * 1000
                    + fraction.parse::<i64>().unwrap(),
            );
            rest = &rest[m.get(0).unwrap().end()..]
        }
        let text = rest
            .chars()
            .map(|c| {
                if c.is_control() || ['\u{2028}', '\u{2029}'].contains(&c) {
                    ' '
                } else {
                    c
                }
            })
            .collect::<String>()
            .trim()
            .to_string();
        if !stamps.is_empty() && text.encode_utf16().count() > 160 {
            return Err("单句歌词超过 160 字符".into());
        }
        for ms in stamps {
            rows.push((ms, text.clone()));
            if rows.len() > 256 {
                return Err("歌词最多 256 个时间点".into());
            }
        }
    }
    if !rows.iter().any(|r| !r.1.is_empty()) {
        return Err("未找到带时间戳的 LRC 歌词".into());
    }
    for r in &mut rows {
        r.0 = (r.0 - offset).max(0)
    }
    rows.sort_by_key(|r| r.0);
    let from = (start * 1000.0).round() as i64;
    let to = (end * 1000.0).round() as i64;
    let mut chosen = Vec::new();
    if !rows.iter().any(|r| r.0 == from) {
        if let Some(r) = rows.iter().rev().find(|r| r.0 < from) {
            if !r.1.is_empty() {
                chosen.push((0, r.1.clone()))
            }
        }
    }
    chosen.extend(
        rows.into_iter()
            .filter(|r| r.0 >= from && r.0 < to)
            .map(|r| (r.0 - from, r.1)),
    );
    if chosen.is_empty() {
        return Err("所选片段内没有歌词，请移除歌词或调整片段".into());
    }
    let result = chosen
        .into_iter()
        .map(|(ms, text)| {
            format!(
                "[{:02}:{:02}.{:03}]{}",
                ms / 60000,
                ms / 1000 % 60,
                ms % 1000,
                text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if result.encode_utf16().count() > 4096 {
        return Err("裁剪后的歌词超过 4096 字符".into());
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clip_shifts_and_carries() {
        assert_eq!(
            clip("[00:01]一\n[00:10]二\n[00:20]三", 5.0, 15.0).unwrap(),
            "[00:00.000]一\n[00:05.000]二"
        )
    }
    #[test]
    fn invalid_lyrics_rejected() {
        assert!(clip("没有时间戳", 0.0, 10.0).is_err());
        assert!(clip("[00:61]错", 0.0, 10.0).is_err())
    }
}
