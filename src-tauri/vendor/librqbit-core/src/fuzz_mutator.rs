//! Fuzzing par mutation, sans dépendance et sur Rust stable (§10.3 du plan d'upgrade) :
//! part d'entrées valides, les abîme au hasard et vérifie que le parseur ne panique jamais.
//! Partagé par les tests de `librqbit-core`, `librqbit-dht` et `librqbit-tracker-comms`
//! (inclus avec `#[path]`). Nombre d'essais : `FUZZ_ITERS` (20 000 par défaut), graine :
//! `FUZZ_SEED` (fixe par défaut, pour qu'un échec soit reproductible).

use std::panic::{AssertUnwindSafe, catch_unwind};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// Octets qui ont un sens pour le bencode : les mutations les visent en priorité.
const TOKENS: &[&[u8]] = &[
    b"d",
    b"l",
    b"e",
    b"i",
    b"i-",
    b":",
    b"0:",
    b"1:",
    b"9999999999:",
    b"i99999999999999999999e",
    b"-",
];

fn mutate(rng: &mut Rng, input: &mut Vec<u8>) {
    for _ in 0..=rng.below(4) {
        let len = input.len();
        match rng.below(7) {
            0 if len > 0 => {
                let i = rng.below(len);
                input[i] = rng.next() as u8;
            }
            1 => {
                let at = rng.below(len + 1);
                input.insert(at, rng.next() as u8);
            }
            2 if len > 0 => {
                let start = rng.below(len);
                let end = (start + 1 + rng.below(16)).min(len);
                input.drain(start..end);
            }
            3 if len > 0 => {
                // duplique un morceau : imbrications et longueurs incohérentes
                let start = rng.below(len);
                let end = (start + 1 + rng.below(64)).min(len);
                let chunk = input[start..end].to_vec();
                let at = rng.below(input.len() + 1);
                input.splice(at..at, chunk);
            }
            4 => {
                let token = TOKENS[rng.below(TOKENS.len())];
                let at = rng.below(len + 1);
                input.splice(at..at, token.iter().copied());
            }
            5 if len > 0 => input.truncate(rng.below(len)),
            _ if len > 0 => {
                // chiffre remplacé : casse les préfixes de longueur
                let i = rng.below(len);
                input[i] = b'0' + rng.below(10) as u8;
            }
            _ => {}
        }
    }
}

fn env_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Appelle `target` sur des mutations de `seeds` ; échoue en montrant l'entrée fautive si
/// `target` panique. Ce que `target` renvoie (Ok ou Err) n'a pas d'importance.
pub fn fuzz(name: &str, seeds: &[&[u8]], target: impl Fn(&[u8])) {
    assert!(!seeds.is_empty());
    let iters = env_or("FUZZ_ITERS", 20_000);
    let mut rng = Rng(env_or("FUZZ_SEED", 0x5EED_CAFE_F00D_1234) | 1);
    // FUZZ_TRACE=fichier : chaque entrée y est écrite avant l'essai, pour retrouver celle qui
    // arrête le processus sans panique (allocation impossible, débordement de pile)
    let trace = std::env::var("FUZZ_TRACE").ok();
    for _ in 0..iters {
        let mut input = seeds[rng.below(seeds.len())].to_vec();
        mutate(&mut rng, &mut input);
        if let Some(path) = &trace {
            std::fs::write(path, &input).unwrap();
        }
        if catch_unwind(AssertUnwindSafe(|| target(&input))).is_err() {
            let hex: String = input.iter().map(|b| format!("{b:02x}")).collect();
            panic!(
                "{name} : panique sur une entrée de {} octets : {hex}",
                input.len()
            );
        }
    }
}
