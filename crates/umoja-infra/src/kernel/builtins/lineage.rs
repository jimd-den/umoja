//! In-Kernel Evolutionary Optimization & Lineage Builtins.

use std::collections::HashMap;
use std::process::Command;
use std::time::Instant;

use rhai::{Array, Dynamic, Engine, Map};
use umoja_domain::lineage::{LineageEntry, ScoreVector};
use umoja_domain::ports::LineageStore;

use crate::sqlite::db::SqliteDb;
use crate::sqlite::stores::SqliteLineageStore;

pub fn register_lineage_builtins(engine: &mut Engine) {
    engine.register_fn("lineage_history", || -> Array {
        get_lineage_history("default", 20)
    });
    engine.register_fn("lineage_history", |target: &str| -> Array {
        get_lineage_history(target, 20)
    });
    engine.register_fn("lineage_history", |target: &str, limit: i64| -> Array {
        get_lineage_history(target, limit.max(1) as usize)
    });

    engine.register_fn("lineage_best", || -> Dynamic {
        get_lineage_best("default")
    });
    engine.register_fn("lineage_best", |target: &str| -> Dynamic {
        get_lineage_best(target)
    });

    engine.register_fn(
        "lineage_commit",
        |target: &str, rationale: &str, score: f64| -> Dynamic {
            commit_lineage_mutation(target, rationale, "tflops", score, true, Map::new())
        },
    );
    engine.register_fn(
        "lineage_commit",
        |target: &str, rationale: &str, metric_name: &str, score: f64, correct: bool| -> Dynamic {
            commit_lineage_mutation(target, rationale, metric_name, score, correct, Map::new())
        },
    );
    engine.register_fn(
        "lineage_commit",
        |target: &str, rationale: &str, metric_name: &str, score: f64, correct: bool, extra_metrics: Map| -> Dynamic {
            commit_lineage_mutation(target, rationale, metric_name, score, correct, extra_metrics)
        },
    );

    engine.register_fn("profile_benchmark", |bench_cmd: &str| -> Dynamic {
        execute_profile_benchmark(bench_cmd)
    });
}

fn open_lineage_store() -> Option<SqliteLineageStore> {
    let home = std::env::var("UMOJA_HOME")
        .or_else(|_| std::env::var("PRIME_AGENT_HOME"))
        .unwrap_or_else(|_| {
            let user_home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{user_home}/.umoja")
        });
    let db_path = format!("{home}/state.db");
    SqliteDb::open(&db_path).ok().map(SqliteLineageStore::new)
}

fn get_lineage_history(target: &str, limit: usize) -> Array {
    let mut arr = Array::new();
    let Some(store) = open_lineage_store() else {
        return arr;
    };

    if let Ok(entries) = store.list(target, limit) {
        for e in entries {
            let mut m = Map::new();
            m.insert("id".into(), Dynamic::from(e.id));
            m.insert("target".into(), Dynamic::from(e.target));
            m.insert("generation".into(), Dynamic::from(e.generation as i64));
            m.insert("rationale".into(), Dynamic::from(e.rationale));
            m.insert("score".into(), Dynamic::from(e.scores.primary_metric));
            m.insert("metric".into(), Dynamic::from(e.scores.primary_metric_name));
            m.insert("correct".into(), Dynamic::from(e.scores.correctness));
            m.insert("commit_hash".into(), Dynamic::from(e.commit_hash.unwrap_or_default()));
            m.insert("created_at".into(), Dynamic::from(e.created_at.to_rfc3339()));
            arr.push(Dynamic::from(m));
        }
    }
    arr
}

fn get_lineage_best(target: &str) -> Dynamic {
    let Some(store) = open_lineage_store() else {
        let mut m = Map::new();
        m.insert("ok".into(), Dynamic::from(false));
        m.insert("error".into(), Dynamic::from("Could not open lineage database".to_string()));
        return Dynamic::from(m);
    };

    let Ok(frontier) = store.pareto_frontier(target) else {
        let mut m = Map::new();
        m.insert("ok".into(), Dynamic::from(false));
        m.insert("error".into(), Dynamic::from("Failed computing Pareto frontier".to_string()));
        return Dynamic::from(m);
    };

    let Some(best) = frontier.best() else {
        let mut m = Map::new();
        m.insert("ok".into(), Dynamic::from(false));
        m.insert("error".into(), Dynamic::from("No lineage entries recorded yet".to_string()));
        return Dynamic::from(m);
    };

    let mut m = Map::new();
    m.insert("ok".into(), Dynamic::from(true));
    m.insert("id".into(), Dynamic::from(best.id.clone()));
    m.insert("target".into(), Dynamic::from(best.target.clone()));
    m.insert("generation".into(), Dynamic::from(best.generation as i64));
    m.insert("rationale".into(), Dynamic::from(best.rationale.clone()));
    m.insert("score".into(), Dynamic::from(best.scores.primary_metric));
    m.insert("metric".into(), Dynamic::from(best.scores.primary_metric_name.clone()));
    m.insert("correct".into(), Dynamic::from(best.scores.correctness));
    m.insert("commit_hash".into(), Dynamic::from(best.commit_hash.clone().unwrap_or_default()));
    Dynamic::from(m)
}

fn commit_lineage_mutation(
    target: &str,
    rationale: &str,
    metric_name: &str,
    score: f64,
    correct: bool,
    extra_metrics: Map,
) -> Dynamic {
    let Some(store) = open_lineage_store() else {
        let mut m = Map::new();
        m.insert("ok".into(), Dynamic::from(false));
        m.insert("error".into(), Dynamic::from("Could not open lineage store".to_string()));
        return Dynamic::from(m);
    };

    let history = store.list(target, 1).unwrap_or_default();
    let gen = history.first().map(|e| e.generation + 1).unwrap_or(1);
    let parent_id = history.first().map(|e| e.id.clone());

    let mut score_vec = ScoreVector::new(metric_name, score, correct);
    for (k, v) in extra_metrics {
        if let Ok(f) = v.as_float() {
            score_vec.metrics.insert(k.to_string(), f);
        } else if let Ok(i) = v.as_int() {
            score_vec.metrics.insert(k.to_string(), i as f64);
        }
    }

    let now_ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let id = format!("lin-{target}-{gen:04}-{now_ts}");
    let mut entry = match LineageEntry::new(&id, target, gen, rationale, score_vec, parent_id) {
        Ok(e) => e,
        Err(err) => {
            let mut m = Map::new();
            m.insert("ok".into(), Dynamic::from(false));
            m.insert("error".into(), Dynamic::from(format!("Failed creating lineage entry: {err}")));
            return Dynamic::from(m);
        }
    };

    // Git commit snapshot if git repository exists
    let commit_out = Command::new("git")
        .args(["commit", "-am", &format!("evo({target}): gen {gen} - {rationale} (score: {score:.2})")])
        .output();

    if let Ok(out) = commit_out {
        if out.status.success() {
            let rev = Command::new("git").args(["rev-parse", "--short", "HEAD"]).output();
            if let Ok(r) = rev {
                entry.commit_hash = Some(String::from_utf8_lossy(&r.stdout).trim().to_string());
            }
        }
    }

    if let Err(e) = store.append(&entry) {
        let mut m = Map::new();
        m.insert("ok".into(), Dynamic::from(false));
        m.insert("error".into(), Dynamic::from(format!("Failed persisting lineage: {e}")));
        return Dynamic::from(m);
    }

    let mut m = Map::new();
    m.insert("ok".into(), Dynamic::from(true));
    m.insert("committed".into(), Dynamic::from(true));
    m.insert("id".into(), Dynamic::from(entry.id));
    m.insert("generation".into(), Dynamic::from(entry.generation as i64));
    m.insert("commit_hash".into(), Dynamic::from(entry.commit_hash.unwrap_or_default()));
    Dynamic::from(m)
}

fn execute_profile_benchmark(bench_cmd: &str) -> Dynamic {
    let start = Instant::now();
    let out = Command::new("sh").arg("-c").arg(bench_cmd).output();

    let mut map = Map::new();
    let elapsed = start.elapsed().as_millis() as f64;
    map.insert("duration_ms".into(), Dynamic::from(elapsed));

    let Ok(output) = out else {
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from("Failed executing benchmark command".to_string()));
        return Dynamic::from(map);
    };

    map.insert("ok".into(), Dynamic::from(output.status.success()));
    map.insert("code".into(), Dynamic::from(output.status.code().unwrap_or(-1) as i64));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = format!("{stdout}\n{stderr}");

    // Parse TFLOPS or latency if present in stdout
    let mut tflops = 0.0f64;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("tflops") {
            for word in line.split_whitespace() {
                if let Ok(val) = word.parse::<f64>() {
                    tflops = val;
                    break;
                }
            }
        }
    }

    map.insert("tflops".into(), Dynamic::from(tflops));
    map.insert("output".into(), Dynamic::from(text));
    Dynamic::from(map)
}
