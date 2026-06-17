// Copyright © 2026 James 'akses' Burger
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License as published by the Free Software Foundation, either version 3 of
// the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
// without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
//
// See the GNU General Public License for more details. You should have received a copy of
// the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
// --------------------------------------------------------- //
// Cellar - Cross-platform GUI for ISO 9660 image creation.  //
// Joliet support for long filenames.                        //
// --------------------------------------------------------- //
// hash.rs - Functions for hashing files.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

/// Hash a file in a background thread. Sends the hex digest (or an error string)
/// down the returned channel exactly once.
pub fn hash_async(path: &Path) -> mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();
    let path = path.to_owned();

    thread::spawn(move || {
        let result = compute(&path);
        let _ = tx.send(result);
    });

    rx
}

fn compute(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hex::encode(hasher.finalize()))
}
