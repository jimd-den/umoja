//! High-performance native dataset and collection operations for Rhai.
//!
//! Replaces interpreted script loops with compiled Rust iterators for operations
//! like aggregation, grouping, sorting, filtering, and extraction.

use rhai::{Array, Dynamic, Engine, Map};
use std::cmp::Ordering;
use std::collections::HashMap;

pub fn register_dataset_builtins(engine: &mut Engine) {
    // -------------------------------------------------------------------------
    // Aggregation: sum_by, avg_by, min_by, max_by
    // -------------------------------------------------------------------------
    engine.register_fn("sum_by", |arr: &mut Array, field: &str| -> f64 {
        sum_field(arr, field)
    });
    engine.register_fn("sum_by", |arr: Array, field: &str| -> f64 {
        sum_field(&arr, field)
    });

    engine.register_fn("avg_by", |arr: &mut Array, field: &str| -> f64 {
        avg_field(arr, field)
    });
    engine.register_fn("avg_by", |arr: Array, field: &str| -> f64 {
        avg_field(&arr, field)
    });

    engine.register_fn("min_by", |arr: &mut Array, field: &str| -> Dynamic {
        min_field(arr, field)
    });
    engine.register_fn("min_by", |arr: Array, field: &str| -> Dynamic {
        min_field(&arr, field)
    });

    engine.register_fn("count_lines", |arr: &mut Array| -> i64 {
        sum_field(arr, "lines") as i64
    });
    engine.register_fn("count_lines", |arr: Array| -> i64 {
        sum_field(&arr, "lines") as i64
    });

    engine.register_fn("total_size", |arr: &mut Array| -> i64 {
        sum_field(arr, "size") as i64
    });
    engine.register_fn("total_size", |arr: Array| -> i64 {
        sum_field(&arr, "size") as i64
    });

    engine.register_fn("filter_by_content", |arr: &mut Array, substring: &str| -> Array {
        filter_string_contains(arr, "content", substring)
    });
    engine.register_fn("filter_by_content", |arr: Array, substring: &str| -> Array {
        filter_string_contains(&arr, "content", substring)
    });

    engine.register_fn("max_by", |arr: &mut Array, field: &str| -> Dynamic {
        max_field(arr, field)
    });
    engine.register_fn("max_by", |arr: Array, field: &str| -> Dynamic {
        max_field(&arr, field)
    });

    // -------------------------------------------------------------------------
    // Grouping & Frequency: count_by, group_by
    // -------------------------------------------------------------------------
    engine.register_fn("count_by", |arr: &mut Array, field: &str| -> Map {
        count_frequency(arr, field)
    });
    engine.register_fn("count_by", |arr: Array, field: &str| -> Map {
        count_frequency(&arr, field)
    });

    engine.register_fn("group_by", |arr: &mut Array, field: &str| -> Map {
        group_items(arr, field)
    });
    engine.register_fn("group_by", |arr: Array, field: &str| -> Map {
        group_items(&arr, field)
    });

    // -------------------------------------------------------------------------
    // Sorting: sort_by, sort_by_desc
    // -------------------------------------------------------------------------
    engine.register_fn("sort_by", |arr: &mut Array, field: &str| -> Array {
        sort_field(arr, field, false)
    });
    engine.register_fn("sort_by", |arr: Array, field: &str| -> Array {
        sort_field(&arr, field, false)
    });

    engine.register_fn("sort_by_desc", |arr: &mut Array, field: &str| -> Array {
        sort_field(arr, field, true)
    });
    engine.register_fn("sort_by_desc", |arr: Array, field: &str| -> Array {
        sort_field(&arr, field, true)
    });

    // -------------------------------------------------------------------------
    // Projection & Deduplication: pluck, unique, unique_by
    // -------------------------------------------------------------------------
    engine.register_fn("pluck", |arr: &mut Array, field: &str| -> Array {
        pluck_field(arr, field)
    });
    engine.register_fn("pluck", |arr: Array, field: &str| -> Array {
        pluck_field(&arr, field)
    });

    engine.register_fn("unique", |arr: &mut Array| -> Array {
        unique_items(arr)
    });
    engine.register_fn("unique", |arr: Array| -> Array {
        unique_items(&arr)
    });

    engine.register_fn("unique_by", |arr: &mut Array, field: &str| -> Array {
        unique_field(arr, field)
    });
    engine.register_fn("unique_by", |arr: Array, field: &str| -> Array {
        unique_field(&arr, field)
    });

    // -------------------------------------------------------------------------
    // Fast Filtering: filter_eq, filter_neq, filter_contains
    // -------------------------------------------------------------------------
    engine.register_fn("filter_eq", |arr: &mut Array, field: &str, val: Dynamic| -> Array {
        filter_equality(arr, field, &val, true)
    });
    engine.register_fn("filter_eq", |arr: Array, field: &str, val: Dynamic| -> Array {
        filter_equality(&arr, field, &val, true)
    });

    engine.register_fn("filter_neq", |arr: &mut Array, field: &str, val: Dynamic| -> Array {
        filter_equality(arr, field, &val, false)
    });
    engine.register_fn("filter_neq", |arr: Array, field: &str, val: Dynamic| -> Array {
        filter_equality(&arr, field, &val, false)
    });

    engine.register_fn("filter_contains", |arr: &mut Array, field: &str, substring: &str| -> Array {
        filter_string_contains(arr, field, substring)
    });
    engine.register_fn("filter_contains", |arr: Array, field: &str, substring: &str| -> Array {
        filter_string_contains(&arr, field, substring)
    });

    // -------------------------------------------------------------------------
    // Search & Slicing: find_first, take_n, drop_n
    // -------------------------------------------------------------------------
    engine.register_fn("find_first", |arr: &mut Array, field: &str, val: Dynamic| -> Dynamic {
        find_first_item(arr, field, &val)
    });
    engine.register_fn("find_first", |arr: Array, field: &str, val: Dynamic| -> Dynamic {
        find_first_item(&arr, field, &val)
    });

    engine.register_fn("take_n", |arr: &mut Array, n: i64| -> Array {
        arr.iter().take(n.max(0) as usize).cloned().collect()
    });
    engine.register_fn("take_n", |arr: Array, n: i64| -> Array {
        arr.iter().take(n.max(0) as usize).cloned().collect()
    });

    engine.register_fn("drop_n", |arr: &mut Array, n: i64| -> Array {
        arr.iter().skip(n.max(0) as usize).cloned().collect()
    });
    engine.register_fn("drop_n", |arr: Array, n: i64| -> Array {
        arr.iter().skip(n.max(0) as usize).cloned().collect()
    });

    // -------------------------------------------------------------------------
    // Type conversion & File ingestion
    // -------------------------------------------------------------------------
    engine.register_fn("parse_int", |s: &str| -> i64 {
        s.trim().parse::<i64>().unwrap_or(0)
    });
    engine.register_fn("parse_float", |s: &str| -> f64 {
        s.trim().parse::<f64>().unwrap_or(0.0)
    });

    engine.register_fn("read_lines", |path: &str| -> Array {
        read_file_lines(path)
    });
    engine.register_fn("parse_csv", |path: &str| -> Array {
        parse_delimited_file(path, 44 as char)
    });
    engine.register_fn("parse_tsv", |path: &str| -> Array {
        parse_delimited_file(path, 9 as char)
    });

    // -------------------------------------------------------------------------
    // Set & Frequency operations: difference, intersect, counter
    // -------------------------------------------------------------------------
    engine.register_fn("difference", |a: Array, b: Array| -> Array {
        array_difference(&a, &b)
    });
    engine.register_fn("difference", |a: &mut Array, b: Array| -> Array {
        array_difference(a, &b)
    });

    engine.register_fn("intersect", |a: Array, b: Array| -> Array {
        array_intersect(&a, &b)
    });
    engine.register_fn("intersect", |a: &mut Array, b: Array| -> Array {
        array_intersect(a, &b)
    });

    engine.register_fn("counter", |a: Array| -> Map {
        array_counter(&a)
    });
    engine.register_fn("counter", |a: &mut Array| -> Map {
        array_counter(a)
    });

    engine.register_fn("join_on", |a: Array, b: Array, key: &str| -> Array {
        array_join_on(&a, &b, key)
    });
    engine.register_fn("join_on", |a: &mut Array, b: Array, key: &str| -> Array {
        array_join_on(a, &b, key)
    });

    engine.register_fn("regex_find_all", |text: &str, pattern: &str| -> Array {
        regex_find_all_matches(text, pattern)
    });
    engine.register_fn("regex_captures", |text: &str, pattern: &str| -> Array {
        regex_capture_groups(text, pattern)
    });
}

// -----------------------------------------------------------------------------
// Helper Implementations
// -----------------------------------------------------------------------------

fn extract_numeric(val: &Dynamic) -> Option<f64> {
    if let Ok(i) = val.as_int() {
        Some(i as f64)
    } else if let Ok(f) = val.as_float() {
        Some(f)
    } else if let Ok(s) = val.clone().into_string() {
        s.trim().parse::<f64>().ok()
    } else {
        None
    }
}

fn sum_field(arr: &[Dynamic], field: &str) -> f64 {
    let mut total = 0.0;
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                if let Some(num) = extract_numeric(val) {
                    total += num;
                }
            }
        }
    }
    total
}

fn avg_field(arr: &[Dynamic], field: &str) -> f64 {
    let mut total = 0.0;
    let mut count = 0usize;
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                if let Some(num) = extract_numeric(val) {
                    total += num;
                    count += 1;
                }
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        total / (count as f64)
    }
}

fn min_field(arr: &[Dynamic], field: &str) -> Dynamic {
    let mut min_val: Option<(f64, Dynamic)> = None;
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                if let Some(num) = extract_numeric(val) {
                    match &min_val {
                        Some((cur_min, _)) if num < *cur_min => {
                            min_val = Some((num, item.clone()));
                        }
                        None => {
                            min_val = Some((num, item.clone()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    min_val.map(|(_, item)| item).unwrap_or(Dynamic::UNIT)
}

fn max_field(arr: &[Dynamic], field: &str) -> Dynamic {
    let mut max_val: Option<(f64, Dynamic)> = None;
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                if let Some(num) = extract_numeric(val) {
                    match &max_val {
                        Some((cur_max, _)) if num > *cur_max => {
                            max_val = Some((num, item.clone()));
                        }
                        None => {
                            max_val = Some((num, item.clone()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    max_val.map(|(_, item)| item).unwrap_or(Dynamic::UNIT)
}

fn count_frequency(arr: &[Dynamic], field: &str) -> Map {
    let mut counts = Map::new();
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                let key = val.to_string();
                let cur = counts.get(key.as_str()).and_then(|v| v.as_int().ok()).unwrap_or(0);
                counts.insert(key.into(), Dynamic::from(cur + 1));
            }
        }
    }
    counts
}

fn group_items(arr: &[Dynamic], field: &str) -> Map {
    let mut groups: HashMap<String, Array> = HashMap::new();
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                let key = val.to_string();
                groups.entry(key).or_default().push(item.clone());
            }
        }
    }
    let mut out = Map::new();
    for (k, v) in groups {
        out.insert(k.into(), Dynamic::from(v));
    }
    out
}

fn sort_field(arr: &[Dynamic], field: &str, desc: bool) -> Array {
    let mut sorted = arr.to_vec();
    sorted.sort_by(|a, b| {
        let val_a = a.clone().try_cast::<Map>().and_then(|m| m.get(field).cloned());
        let val_b = b.clone().try_cast::<Map>().and_then(|m| m.get(field).cloned());

        let cmp = match (val_a, val_b) {
            (Some(va), Some(vb)) => {
                if let (Some(na), Some(nb)) = (extract_numeric(&va), extract_numeric(&vb)) {
                    na.partial_cmp(&nb).unwrap_or(Ordering::Equal)
                } else {
                    va.to_string().cmp(&vb.to_string())
                }
            }
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        };

        if desc {
            cmp.reverse()
        } else {
            cmp
        }
    });
    sorted
}

fn pluck_field(arr: &[Dynamic], field: &str) -> Array {
    let mut result = Array::new();
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                result.push(val.clone());
            }
        }
    }
    result
}

fn unique_items(arr: &[Dynamic]) -> Array {
    let mut seen = std::collections::HashSet::new();
    let mut result = Array::new();
    for item in arr {
        let key = item.to_string();
        if seen.insert(key) {
            result.push(item.clone());
        }
    }
    result
}

fn unique_field(arr: &[Dynamic], field: &str) -> Array {
    let mut seen = std::collections::HashSet::new();
    let mut result = Array::new();
    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                let key = val.to_string();
                if seen.insert(key) {
                    result.push(item.clone());
                }
            }
        }
    }
    result
}

fn filter_equality(arr: &[Dynamic], field: &str, target_val: &Dynamic, equals: bool) -> Array {
    let mut result = Array::new();
    let target_str = target_val.to_string();
    let target_num = extract_numeric(target_val);

    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                let is_match = if let (Some(n1), Some(n2)) = (extract_numeric(val), target_num) {
                    (n1 - n2).abs() < f64::EPSILON
                } else {
                    val.to_string() == target_str
                };

                if is_match == equals {
                    result.push(item.clone());
                }
            }
        }
    }
    result
}

fn filter_string_contains(arr: &[Dynamic], field: &str, substring: &str) -> Array {
    let mut result = Array::new();
    let sub_lower = substring.to_lowercase();

    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                if val.to_string().to_lowercase().contains(&sub_lower) {
                    result.push(item.clone());
                }
            }
        }
    }
    result
}

fn find_first_item(arr: &[Dynamic], field: &str, target_val: &Dynamic) -> Dynamic {
    let target_str = target_val.to_string();
    let target_num = extract_numeric(target_val);

    for item in arr {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(field) {
                let is_match = if let (Some(n1), Some(n2)) = (extract_numeric(val), target_num) {
                    (n1 - n2).abs() < f64::EPSILON
                } else {
                    val.to_string() == target_str
                };

                if is_match {
                    return item.clone();
                }
            }
        }
    }
    Dynamic::UNIT
}

fn read_file_lines(path: &str) -> Array {
    let mut arr = Array::new();
    if let Ok(content) = std::fs::read_to_string(path) {
        for line in content.lines() {
            arr.push(Dynamic::from(line.to_string()));
        }
    }
    arr
}

fn parse_delimited_file(path: &str, delimiter: char) -> Array {
    let mut rows = Array::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return rows;
    };
    let mut lines = content.lines();
    let Some(header_line) = lines.next() else {
        return rows;
    };
    let headers: Vec<String> = header_line
        .split(delimiter)
        .map(|h| h.trim().trim_matches(|c: char| c == 34 as char).to_string())
        .collect();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(delimiter).collect();
        let mut map = Map::new();
        for (i, header) in headers.iter().enumerate() {
            let val = cols.get(i).map(|c| c.trim().trim_matches(|c: char| c == 34 as char)).unwrap_or("");
            map.insert(header.clone().into(), Dynamic::from(val.to_string()));
        }
        rows.push(Dynamic::from(map));
    }
    rows
}

fn array_difference(a: &[Dynamic], b: &[Dynamic]) -> Array {
    let b_set: std::collections::HashSet<String> = b.iter().map(|x| x.to_string()).collect();
    let mut result = Array::new();
    for item in a {
        if !b_set.contains(&item.to_string()) {
            result.push(item.clone());
        }
    }
    result
}

fn array_intersect(a: &[Dynamic], b: &[Dynamic]) -> Array {
    let b_set: std::collections::HashSet<String> = b.iter().map(|x| x.to_string()).collect();
    let mut seen = std::collections::HashSet::new();
    let mut result = Array::new();
    for item in a {
        let s = item.to_string();
        if b_set.contains(&s) && seen.insert(s) {
            result.push(item.clone());
        }
    }
    result
}

fn array_counter(a: &[Dynamic]) -> Map {
    let mut map = Map::new();
    for item in a {
        let key = item.to_string();
        let curr = map.get(key.as_str()).and_then(|v| v.clone().try_cast::<i64>()).unwrap_or(0);
        map.insert(key.into(), Dynamic::from(curr + 1));
    }
    map
}

fn array_join_on(a: &[Dynamic], b: &[Dynamic], key: &str) -> Array {
    let mut b_by_key: HashMap<String, Map> = HashMap::new();
    for item in b {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let Some(val) = m.get(key) {
                b_by_key.insert(val.to_string(), m);
            }
        }
    }

    let mut result = Array::new();
    for item in a {
        if let Some(mut m_a) = item.clone().try_cast::<Map>() {
            if let Some(val) = m_a.get(key) {
                if let Some(m_b) = b_by_key.get(&val.to_string()) {
                    for (k, v) in m_b {
                        if !m_a.contains_key(k) {
                            m_a.insert(k.clone(), v.clone());
                        }
                    }
                    result.push(Dynamic::from(m_a));
                }
            }
        }
    }
    result
}

fn regex_find_all_matches(text: &str, pattern: &str) -> Array {
    let mut result = Array::new();
    if let Ok(re) = regex::Regex::new(pattern) {
        for m in re.find_iter(text) {
            result.push(Dynamic::from(m.as_str().to_string()));
        }
    }
    result
}

fn regex_capture_groups(text: &str, pattern: &str) -> Array {
    let mut result = Array::new();
    if let Ok(re) = regex::Regex::new(pattern) {
        for caps in re.captures_iter(text) {
            let mut row = Array::new();
            for m in caps.iter() {
                let s = m.map(|m| m.as_str().to_string()).unwrap_or_default();
                row.push(Dynamic::from(s));
            }
            result.push(Dynamic::from(row));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dataset_aggregations_and_grouping() {
        let mut engine = Engine::new();
        register_dataset_builtins(&mut engine);

        let script = r#"
            let users = [
                #{ name: "Alice", dept: "Eng", score: 90 },
                #{ name: "Bob", dept: "Sales", score: 70 },
                #{ name: "Charlie", dept: "Eng", score: 80 }
            ];

            let total = users.sum_by("score");
            let average = users.avg_by("score");
            let top_user = users.max_by("score");
            let dept_counts = users.count_by("dept");
            let eng_users = users.filter_eq("dept", "Eng");

            print("Total: " + total + ", Avg: " + average + ", Eng: " + eng_users.len());
            total == 240.0 && average == 80.0 && eng_users.len() == 2
        "#;

        let res = engine.eval::<bool>(script).unwrap();
        assert!(res);
    }

    #[test]
    fn test_dataset_parsing_sets_and_conversions() {
        let mut engine = Engine::new();
        register_dataset_builtins(&mut engine);

        // 1. parse_int & parse_float
        let val_i: i64 = engine.eval(r#"parse_int("42")"#).unwrap();
        assert_eq!(val_i, 42);
        let val_f: f64 = engine.eval(r#"parse_float("3.14")"#).unwrap();
        assert!((val_f - 3.14).abs() < 1e-5);

        // 2. read_lines & parse_csv
        let csv_path = "/tmp/umoja_test_dataset.csv";
        std::fs::write(csv_path, "id,name,score\n1,Alice,95\n2,Bob,80\n").unwrap();

        let lines: Array = engine.eval(&format!(r#"read_lines("{csv_path}")"#)).unwrap();
        assert_eq!(lines.len(), 3);

        let rows: Array = engine.eval(&format!(r#"parse_csv("{csv_path}")"#)).unwrap();
        assert_eq!(rows.len(), 2);
        let first_row = rows[0].clone().cast::<Map>();
        assert_eq!(first_row.get("name").unwrap().to_string(), "Alice");
        assert_eq!(first_row.get("score").unwrap().to_string(), "95");

        let _ = std::fs::remove_file(csv_path);

        // 3. difference, intersect, counter
        let diff: Array = engine.eval(r#"difference([1, 2, 3], [2, 3, 4])"#).unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].clone().cast::<i64>(), 1);

        let inter: Array = engine.eval(r#"intersect([1, 2, 3], [2, 3, 4])"#).unwrap();
        assert_eq!(inter.len(), 2);

        let counts: Map = engine.eval(r#"counter(["apple", "banana", "apple"])"#).unwrap();
        assert_eq!(counts.get("apple").unwrap().clone().cast::<i64>(), 2);
        assert_eq!(counts.get("banana").unwrap().clone().cast::<i64>(), 1);

        // 4. regex_find_all, regex_captures, join_on
        let regex_matches: Array = engine.eval(r#"regex_find_all("ab ab", "ab")"#).unwrap();
        assert_eq!(regex_matches.len(), 2);

        let joined: Array = engine.eval(r#"join_on([#{id: 1, name: "Alice"}], [#{id: 1, role: "Admin"}], "id")"#).unwrap();
        assert_eq!(joined.len(), 1);
        let first_j = joined[0].clone().cast::<Map>();
        assert_eq!(first_j.get("name").unwrap().to_string(), "Alice");
        assert_eq!(first_j.get("role").unwrap().to_string(), "Admin");
    }
}

