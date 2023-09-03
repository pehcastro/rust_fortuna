use std::{sync::{atomic::{AtomicBool, Ordering}, Arc, mpsc}, thread, time::Duration};

use cardano_multiplatform_lib::{self, plutus::{encode_json_str_to_plutus_datum, PlutusDatumSchema}};
use serde::Serialize;
use sha2::{Sha256, Digest};
use regex::Regex;
use rand::RngCore;
use reqwest::{self, Error};

fn main() {

    let mut json = String::from(r#"{"fields":[{"bytes":"2c58571ed350979233da4dea0846ff50"},{"int":7745},{"bytes":"0000003ae1a0f5771417fe0ca2865f93a0ff6d2534037570fc5e258ca9213845"},{"int":66},{"int":555},{"int":61545000}],"constructor":0}"#);
    let mut solved = false;
    const NUM_THREADS: usize = 16;
    
    let (tx, rx) = mpsc::channel();
    let should_terminate = Arc::new(AtomicBool::new(false));

    loop {
        let mut handles = vec![];
        let json_update = fetch_url("http://24.144.88.111:9608").unwrap().clone();

        if solved && json_update == json {
            continue;
        }

        solved = false;

        println!("Updated! New hashing cycle beginning now.");

        for i in 0..NUM_THREADS {
            let should_terminate = Arc::clone(&should_terminate);
            let tx = tx.clone();
            let clone_json = json.clone();
            let handle = thread::spawn(move || {
                worker(i, clone_json.as_str(), should_terminate, tx);
            });
            handles.push(handle);
        }

        loop {
            // Non-blocking check if a worker found a solution
            match rx.try_recv() {
                Ok(answer) => {
                    println!("Found a solution!");
                    post_answer("http://24.144.88.111:9608/submit", &answer).unwrap();
                    solved = true;
                    break;
                },
                Err(mpsc::TryRecvError::Empty) => {
                    // No worker has found a solution yet. Continue polling or doing other tasks.
                    let new_json = fetch_url("http://24.144.88.111:9608").unwrap().clone();
                    if new_json != json {
                        println!("External source indicated to restart jobs.");
                        json = new_json;
                        break;
                    }
                },
                Err(mpsc::TryRecvError::Disconnected) => panic!("Channel disconnected"),
            }
            thread::sleep(Duration::from_millis(100)); // Optional, to avoid busy-waiting
        }

        // Reset for the next iteration
        should_terminate.store(true, Ordering::Relaxed);
        for handle in handles {
            handle.join().unwrap();
        }
        should_terminate.store(false, Ordering::Relaxed);
    }

}


#[derive(Serialize)]
struct FoundAnswerResponse {
    nonce: Vec<u8>,
    answer: Vec<u8>,
    difficulty: u128,
    zeroes: u128
}

fn worker(_id: usize, json: &str, should_terminate: Arc<AtomicBool>, tx: mpsc::Sender<FoundAnswerResponse>) {
    let fields = extract_fields(json);

    let Ok(result) = encode_json_str_to_plutus_datum(json, PlutusDatumSchema::DetailedSchema) else { panic!("k1")};

    let Some(constr) = result.as_constr_plutus_data() else { panic!("k1.5")};

    let mut bytes = constr.to_bytes();

    let FieldValue::Int(expected_zeroes) = &fields[3] else { panic!()};
    let FieldValue::Int(expected_diff) = &fields[4] else { panic!()};
    let mut iters = 0u32;
    let mut rng = rand::thread_rng();
    let mut nonce = [0u8; 16];
    rng.fill_bytes(&mut nonce);
    bytes[4..20].copy_from_slice(&nonce);
    
    while !should_terminate.load(Ordering::Relaxed) {
        let hashed_data = sha256_digest_as_bytes(&bytes);
        let hashed_hash = sha256_digest_as_bytes(&hashed_data);
        let (zeroes, difficulty) = get_difficulty(&hashed_hash);
        iters = iters + 1u32;
        if zeroes > *expected_zeroes as u128 || (zeroes == *expected_zeroes as u128 && difficulty  < *expected_diff  as u128) {
            tx.send(FoundAnswerResponse { 
                nonce: bytes[4..20].to_vec(), 
                answer: hashed_hash.to_vec(),
                difficulty: difficulty,
                zeroes: zeroes,
            }).unwrap();
            break;
        }
        increment_u8_array(&mut bytes[4..20])
    }
}

fn sha256_digest_as_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let arr: [u8; 32] = result.into();
    arr
}

pub fn get_difficulty(hash: &[u8]) -> (u128, u128) {
    // If you want to check that the Vec is the expected length:
    if hash.len() != 32 {
        panic!("Expected a hash of length 32, but got {}", hash.len());
    }

    let mut leading_zeros = 0;
    let mut difficulty_number = 0;

    for (indx, &chr) in hash.iter().enumerate() {
        if chr != 0 {
            if (chr & 0x0F) == chr {
                leading_zeros += 1;
                difficulty_number += (chr as u128) * 4096;
                difficulty_number += (hash[indx + 1] as u128) * 16;
                difficulty_number += (hash[indx + 2] as u128) / 16;
                return (leading_zeros, difficulty_number);
            } else {
                difficulty_number += (chr as u128) * 256;
                difficulty_number += hash[indx + 1] as u128;
                return (leading_zeros, difficulty_number);
            }
        } else {
            leading_zeros += 2;
        }
    }
    (32, 0)
}

pub fn increment_u8_array(x: &mut [u8]) {
    for byte in x.iter_mut() {
        if *byte == 255 {
            *byte = 0;
        } else {
            *byte += 1;
            break;
        }
    }
}

#[derive(Debug)]
enum FieldValue {
    Bytes(String),
    Int(i32),
}

fn extract_fields(input: &str) -> Vec<FieldValue> {
    let re = Regex::new(r#"(?:"bytes":"([^"]+)"|"int":(\d+))"#).unwrap();
    re.captures_iter(input)
        .filter_map(|cap| {
            if let Some(val) = cap.get(1) {
                Some(FieldValue::Bytes(val.as_str().to_string()))
            } else {
                cap.get(2).map(|val| FieldValue::Int(val.as_str().parse().unwrap()))
            }
        })
        .collect()
}

fn fetch_url(url: &str) -> Result<String, reqwest::Error> {
    let body = reqwest::blocking::get(url)?.text()?;
    Ok(body)
}

fn post_answer(url: &str, answer: &FoundAnswerResponse) -> Result<(), Error> {
    let client = reqwest::blocking::Client::new();
    let _res = client.post(url)
        .json(answer)
        .send()?;

    Ok(())
}