// Scratch: bytes in the database by kind of key, objects by type.
use std::collections::BTreeMap;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("store dir"));
    let schema = rhypedb_schema::parser::parse_schema(include_str!("../src/schema.rhype")).unwrap();
    let db = rhypedb_engine::database::Database::open(schema, dir.join("db")).unwrap();
    let storage = db.storage();
    let snap = storage.read_snapshot();
    let names: BTreeMap<u64, String> = db
        .type_ids()
        .iter()
        .map(|(n, id)| (*id, n.clone()))
        .collect();
    let mb = |b: usize| b as f64 / 1_048_576.0;
    for (p, label) in [
        (b'o', "objects"),
        (b'e', "edges"),
        (b'r', "reverse edges"),
        (b'f', "fulltext postings"),
        (b'l', "fulltext docs"),
        (b'u', "unique index"),
        (b'i', "field index"),
        (b'g', "generations"),
        (b'c', "catalog"),
    ] {
        let rows = storage.scan_prefix_at(snap, &[p, b':']).unwrap();
        let (k, v): (usize, usize) = rows
            .iter()
            .fold((0, 0), |a, (k, v)| (a.0 + k.len(), a.1 + v.len()));
        println!(
            "{label:18} {:8} rows  keys {:7.1} MB  values {:7.1} MB",
            rows.len(),
            mb(k),
            mb(v)
        );
        if p == b'o' {
            let mut by_type: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for (k, v) in &rows {
                let t = u64::from_be_bytes(k[2..10].try_into().unwrap());
                let e = by_type
                    .entry(names.get(&t).cloned().unwrap_or(t.to_string()))
                    .or_default();
                e.0 += 1;
                e.1 += v.len();
            }
            for (t, (n, b)) in by_type {
                println!("    {t:12} {n:8}  {:7.1} MB", mb(b));
            }
        }
    }
}
