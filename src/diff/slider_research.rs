//! Research tools for finding out where Git places a group of changed
//! lines that could sit in several places (a "slider"), from Git's output
//! only. These are not tests of zerogit: they are `#[ignore]`d and run on
//! demand, writing to the directory in `SLIDER_OUT`:
//!
//! ```text
//! SLIDER_OUT=dir cargo test --lib slider_dataset -- --ignored
//! SLIDER_OUT=dir SLIDER_FEATURES=... cargo test --lib slider_fit -- --ignored --nocapture
//! ```
//!
//! See `docs/diff-compat-research.md`, finding 3.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use super::compat_tests::{git_headers, Rng};
use super::histogram::Algorithm;

/// The indentation of a line in columns (tab stops every 8), or -1 if it
/// is blank.
fn indent(line: &str) -> i64 {
    let mut columns = 0;
    for c in line.chars() {
        match c {
            ' ' => columns += 1,
            '\t' => columns += 8 - columns % 8,
            '\n' | '\r' => {}
            _ => return columns,
        }
    }
    -1
}

/// What is around a boundary just before line `s`: the indentation of
/// line `s` (-1 if blank, -2 past the end), the blank lines just before
/// it and the indentation of the nearest non-blank line before them (-1
/// if none), and the blank lines just after line `s` and the indentation
/// of the nearest non-blank line after them (-1 if none).
fn boundary(lines: &[&str], s: usize) -> [i64; 5] {
    let at = lines.get(s).map_or(-2, |l| indent(l));
    let pre_blank = lines[..s]
        .iter()
        .rev()
        .take_while(|l| indent(l) == -1)
        .count() as i64;
    let pre = lines[..s]
        .iter()
        .rev()
        .map(|l| indent(l))
        .find(|&i| i != -1)
        .unwrap_or(-1);
    let rest = lines.get(s + 1..).unwrap_or(&[]);
    let post_blank = rest.iter().take_while(|l| indent(l) == -1).count() as i64;
    let post = rest
        .iter()
        .map(|l| indent(l))
        .find(|&i| i != -1)
        .unwrap_or(-1);
    [at, pre_blank, pre, post_blank, post]
}

/// A position: its shift, whether Git chose it, its penalty features and
/// its indentation.
type Candidate = (i64, bool, Vec<i64>, i64);

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var("SLIDER_OUT").expect("SLIDER_OUT"))
}

/// Random single insertions (a slice of the file repeated next to itself,
/// so that the inserted group can slide), the positions the group could
/// take and the one Git chooses, with the facts about both boundaries of
/// each position.
#[test]
#[ignore]
fn slider_dataset() {
    let cases: usize = std::env::var("SLIDER_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(500);
    let seed: u64 = std::env::var("SLIDER_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5851F42D4C957F2D);
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path();
    let mut rng = Rng(seed);
    let vocabulary = [
        "\n",
        "\n",
        "a\n",
        "  a\n",
        "    a\n",
        "\tb\n",
        "  b\n",
        "}\n",
        "  }\n",
        "c\n",
        "    c\n",
        "      d\n",
    ];
    let mut table = String::from("case\tshift\tchosen\tsize\ttop\tbottom\n");
    for case in 0..cases {
        let old: Vec<&str> = (0..3 + rng.below(16))
            .map(|_| vocabulary[rng.below(vocabulary.len() as u64) as usize])
            .collect();
        let start = rng.below(old.len() as u64) as usize;
        let end = start + 1 + rng.below((old.len() - start).min(5) as u64) as usize;
        let mut new = old.clone();
        let copy: Vec<&str> = old[start..end].to_vec();
        new.splice(end..end, copy);
        let size = end - start;
        let (mut g_start, mut g_end) = (end, end + size);
        while g_end < new.len() && new[g_start] == new[g_end] {
            g_start += 1;
            g_end += 1;
        }
        let lowest_end = g_end;
        while g_start > 0 && new[g_start - 1] == new[g_end - 1] {
            g_start -= 1;
            g_end -= 1;
        }
        let highest_end = g_end;
        std::fs::write(dir.join("old"), old.concat()).unwrap();
        std::fs::write(dir.join("new"), new.concat()).unwrap();
        let theirs = git_headers(dir, "old", "new", Algorithm::Myers);
        if theirs.len() != 1 {
            continue;
        }
        let plus = theirs[0].split('+').nth(1).unwrap();
        let (s, len) = plus.split_once(',').unwrap_or((plus, "1"));
        let (s, len): (usize, usize) = (s.parse().unwrap(), len.parse().unwrap());
        if len != size {
            continue;
        }
        let chosen_end = s - 1 + len;
        for g_end in highest_end..=lowest_end {
            let fmt = |b: [i64; 5]| {
                b.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            writeln!(
                table,
                "{}\t{}\t{}\t{}\t{}\t{}",
                case,
                lowest_end - g_end,
                u8::from(g_end == chosen_end),
                size,
                fmt(boundary(&new, g_end - size)),
                fmt(boundary(&new, g_end)),
            )
            .unwrap();
        }
    }
    std::fs::write(out_dir().join("slider_dataset.tsv"), table).unwrap();
}

/// The penalty features of one boundary, for the feature set named in
/// `SLIDER_FEATURES` (comma-separated), and its indentation.
fn features(b: [i64; 5], set: &[String]) -> (Vec<i64>, i64) {
    let [at, pre_blank, pre, post_blank, post] = b;
    let at_end = at == -2;
    let blank_at = at == -1;
    // Blank lines on the boundary's far side count from the line at it.
    let after_blank = if blank_at { 1 + post_blank } else { 0 };
    let total_blank = pre_blank + after_blank;
    let ind = if at >= 0 { at } else { post };
    let any_blank = total_blank > 0;
    let mut v = Vec::new();
    for name in set {
        let x = match name.as_str() {
            "start" => i64::from(pre == -1 && pre_blank == 0),
            "end" => i64::from(at_end),
            "pre_blank" => pre_blank,
            "after_blank" => after_blank,
            "total_blank" => total_blank,
            "in" => i64::from(ind != -1 && pre != -1 && ind > pre && !any_blank),
            "in_blank" => i64::from(ind != -1 && pre != -1 && ind > pre && any_blank),
            "out" => i64::from(ind != -1 && pre != -1 && ind < pre && !any_blank),
            "out_blank" => i64::from(ind != -1 && pre != -1 && ind < pre && any_blank),
            "out_up" => i64::from(
                ind != -1 && pre != -1 && ind < pre && !any_blank && post != -1 && post > ind,
            ),
            "out_up_blank" => i64::from(
                ind != -1 && pre != -1 && ind < pre && any_blank && post != -1 && post > ind,
            ),
            other => panic!("unknown feature {}", other),
        };
        v.push(x);
    }
    (v, ind)
}

/// Fits weights so that, for each case, Git's chosen position beats every
/// other: `W * sign(indent(other) - indent(chosen)) + penalty(other) -
/// penalty(chosen) > 0`, or `>= 0` for positions Git would reach later
/// (ties). Reports how many cases the best weights explain.
#[test]
#[ignore]
fn slider_fit() {
    let set: Vec<String> = std::env::var("SLIDER_FEATURES")
        .unwrap_or_else(|_| "start,end,pre_blank,after_blank,in,out".to_owned())
        .split(',')
        .map(str::to_owned)
        .collect();
    let text = std::fs::read_to_string(out_dir().join("slider_dataset.tsv")).unwrap();
    // Per case: (shift, chosen, penalty features, indentation).
    let mut cases: BTreeMap<u64, Vec<Candidate>> = BTreeMap::new();
    let parse = |s: &str| -> [i64; 5] {
        let v: Vec<i64> = s.split(',').map(|x| x.parse().unwrap()).collect();
        [v[0], v[1], v[2], v[3], v[4]]
    };
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        // Only positions at most `size + SLIDER_LIMIT` above the lowest.
        let (shift, size): (i64, i64) = (f[1].parse().unwrap(), f[3].parse().unwrap());
        if let Some(limit) = std::env::var("SLIDER_LIMIT")
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
        {
            if shift > size + limit {
                continue;
            }
        }
        let (top, top_ind) = features(parse(f[4]), &set);
        let (bottom, bottom_ind) = features(parse(f[5]), &set);
        let v: Vec<i64> = top.iter().zip(&bottom).map(|(a, b)| a + b).collect();
        cases.entry(f[0].parse().unwrap()).or_default().push((
            f[1].parse().unwrap(),
            f[2] == "1",
            v,
            top_ind + bottom_ind,
        ));
    }
    let tie: i64 = std::env::var("SLIDER_TIE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    // Pairwise difference vectors: [sign of indentation difference,
    // penalty differences...]; each must score > 0 (or >= 0 when Git
    // would prefer `chosen` on a tie).
    let mut pairs: Vec<(u64, Vec<i64>, bool)> = Vec::new();
    for (&case, candidates) in &cases {
        let chosen = candidates.iter().find(|c| c.1).unwrap();
        for other in candidates.iter().filter(|c| !c.1) {
            let mut d = vec![(other.3 - chosen.3).signum()];
            d.extend(other.2.iter().zip(&chosen.2).map(|(o, c)| o - c));
            // With SLIDER_TIE=1 a tie goes to the lower position (larger
            // shift = higher up), with -1 to the higher one.
            let strict = (other.0 - chosen.0) * tie > 0;
            pairs.push((case, d, strict));
        }
    }
    let dim = set.len() + 1;
    let mut w = vec![0i64; dim];
    let dot = |w: &[i64], d: &[i64]| -> i64 { w.iter().zip(d).map(|(a, b)| a * b).sum() };
    let explained = |w: &[i64]| -> usize {
        let mut bad = std::collections::BTreeSet::new();
        for (case, d, strict) in &pairs {
            let s = dot(w, d);
            if s < 0 || (*strict && s == 0) {
                bad.insert(*case);
            }
        }
        cases.len() - bad.len()
    };
    let mut best = (0, w.clone());
    for _ in 0..500 {
        let mut changed = false;
        for (_, d, strict) in &pairs {
            let s = dot(&w, d);
            if s < 0 || (*strict && s == 0) {
                for k in 0..dim {
                    w[k] += d[k];
                }
                changed = true;
            }
        }
        let e = explained(&w);
        if e > best.0 {
            best = (e, w.clone());
        }
        if !changed {
            break;
        }
    }
    // Sequential choice: positions from the highest down, each replacing
    // the best so far unless it compares worse (`cmp > 0`).
    if std::env::var("SLIDER_SEQ").is_ok() {
        let cmp = |w: &[i64], x: &Candidate, y: &Candidate| -> i64 {
            let mut d = vec![(x.3 - y.3).signum()];
            d.extend(x.2.iter().zip(&y.2).map(|(a, b)| a - b));
            dot(w, &d)
        };
        let choose = |w: &[i64], candidates: &[Candidate]| -> usize {
            let mut order: Vec<usize> = (0..candidates.len()).collect();
            order.sort_by_key(|&i| -candidates[i].0);
            let mut best = order[0];
            for &i in &order[1..] {
                if cmp(w, &candidates[i], &candidates[best]) <= 0 {
                    best = i;
                }
            }
            best
        };
        let mut w = best.1.clone();
        let mut best_seq = (0usize, w.clone());
        for _ in 0..300 {
            let mut correct = 0;
            for candidates in cases.values() {
                let predicted = choose(&w, candidates);
                if candidates[predicted].1 {
                    correct += 1;
                    continue;
                }
                let chosen = candidates.iter().find(|c| c.1).unwrap();
                let p = &candidates[predicted];
                // Make the chosen one compare better than the predicted.
                let mut d = vec![(p.3 - chosen.3).signum()];
                d.extend(p.2.iter().zip(&chosen.2).map(|(a, b)| a - b));
                for k in 0..dim {
                    w[k] += d[k];
                }
            }
            if correct > best_seq.0 {
                best_seq = (correct, w.clone());
            }
            if correct == cases.len() {
                break;
            }
        }
        // Local search from the perceptron's weights: random integer steps
        // that do not lose cases.
        let count = |w: &[i64]| -> usize { cases.values().filter(|c| c[choose(w, c)].1).count() };
        // Start from given weights if any (SLIDER_START=w0,w1,...).
        if let Ok(start) = std::env::var("SLIDER_START") {
            best_seq.1 = start.split(',').map(|x| x.parse().unwrap()).collect();
            best_seq.0 = count(&best_seq.1);
        }
        let mut rng = Rng(0x2545F4914F6CDD1D);
        let steps: usize = std::env::var("SLIDER_STEPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20_000);
        for _ in 0..steps {
            if best_seq.0 == cases.len() {
                break;
            }
            let mut w = best_seq.1.clone();
            for _ in 0..1 + rng.below(2) {
                let k = rng.below(dim as u64) as usize;
                w[k] += rng.below(11) as i64 - 5;
            }
            let c = count(&w);
            if c >= best_seq.0 {
                best_seq = (c, w);
            }
        }
        eprintln!(
            "sequential: explained {} of {} cases; weights {:?}",
            best_seq.0,
            cases.len(),
            best_seq.1
        );
        let w = best_seq.1.clone();
        for (case, candidates) in &cases {
            let predicted = choose(&w, candidates);
            if !candidates[predicted].1 {
                eprintln!(
                    "  misfit case {}: predicted shift {}, Git shift {}",
                    case,
                    candidates[predicted].0,
                    candidates.iter().find(|c| c.1).unwrap().0
                );
                for line in text.lines().skip(1) {
                    if line.split('\t').next() == Some(&case.to_string()) {
                        eprintln!("    {}", line);
                    }
                }
            }
        }
    }
    // Optionally, show the cases the best weights do not explain.
    let show: usize = std::env::var("SLIDER_SHOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if show > 0 {
        let rows: BTreeMap<u64, Vec<&str>> =
            text.lines().skip(1).fold(BTreeMap::new(), |mut m, l| {
                m.entry(l.split('\t').next().unwrap().parse().unwrap())
                    .or_insert_with(Vec::new)
                    .push(l);
                m
            });
        let mut shown = 0;
        for (case, d, strict) in &pairs {
            let s = dot(&best.1, d);
            if (s < 0 || (*strict && s == 0)) && shown < show {
                shown += 1;
                eprintln!("case {} (pair score {}, strict {}):", case, s, strict);
                for row in &rows[case] {
                    eprintln!("  {}", row);
                }
            }
        }
    }
    let mut names = vec!["INDENT_SIGN".to_owned()];
    names.extend(set.iter().cloned());
    eprintln!(
        "explained {} of {} cases; weights: {}",
        best.0,
        cases.len(),
        names
            .iter()
            .zip(&best.1)
            .map(|(n, w)| format!("{}={}", n, w))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
