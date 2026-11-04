// Local protocol test driver. No device or AstroBox APIs.
use huanjian_music_plugin::transfer::Sender;
use serde_json::Value;
use std::io::{self, BufRead};
fn main() {
    let mut s = Sender::default();
    for line in io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&line.unwrap()).unwrap();
        match v["command"].as_str().unwrap() {
            "start" => s
                .start(
                    v["id"].as_str().unwrap().into(),
                    "device".into(),
                    v["title"].as_str().unwrap().into(),
                    serde_json::from_value(v["bytes"].clone()).unwrap(),
                    v["lyrics"].as_str().unwrap_or("").into(),
                    v["profile"].as_str().unwrap_or("balanced"),
                    v["peak"].as_u64().unwrap_or(64).try_into().unwrap(),
                )
                .unwrap(),
            "ack" => s.receive(v["ack"].clone()),
            "timer" => {
                let kind = s.due.as_ref().unwrap().1.clone();
                s.timeout(s.serial, &kind)
            }
            "resume" => s.resume().unwrap(),
            "cancel" => s.cancel(),
            _ => panic!("unknown command"),
        };
        println!("{}", s.snapshot())
    }
}
