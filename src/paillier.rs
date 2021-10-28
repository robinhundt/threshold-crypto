use crate::rand::{generate_safe_prime, random_in_mult_group};
use crate::util;
use anyhow::{anyhow, Result};
use rug::rand::MutRandState;
use rug::{Assign, Complete, Integer};
use serde::{Deserialize, Serialize};

use std::convert::TryInto;
use std::thread;

// WIP
// pub struct Ciphertext<'a> {
//     pk: &'a PublicKey,
//     val: Integer,
// }
//
// pub struct Plaintext {
//     val: Integer,
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateKeyShare {
    i: u32,
    /// Polynomial evaluation at i
    #[serde(with = "crate::util::serde_integer")]
    si: Integer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialDecryption {
    #[serde(with = "crate::util::serde_integer")]
    val: Integer,
    id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct PublicKey {
    /// The number of servers req to successfully decrypt
    w: u32,
    /// The number of decryption servers in total
    l: u32,
    /// Modulus of the key. n = p * q
    #[serde(with = "crate::util::serde_integer")]
    n: Integer,
    /// Precomputation: n + 1
    #[serde(with = "crate::util::serde_integer")]
    g: Integer,
    /// Precomputation: n^2
    #[serde(with = "crate::util::serde_integer")]
    n2: Integer,
    /// Precomputation: l!
    #[serde(with = "crate::util::serde_integer")]
    delta: Integer,
    /// Precomputation (4*delta^2)^{-1} mod n
    #[serde(with = "crate::util::serde_integer")]
    combine_shares_constant: Integer,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct PrivateKey {
    /// The number of servers req to decrypt
    w: u32,
    /// The number of decryption servers in total
    l: u32,
    /// d = 0 mod m and d = 1 mod n^2
    #[serde(with = "crate::util::serde_integer")]
    d: Integer,
    /// Modulus of the key: p * q
    #[serde(with = "crate::util::serde_integer")]
    n: Integer,
    /// Precomputation: n^2
    #[serde(with = "crate::util::serde_integer")]
    n2: Integer,
    /// Precomputation: n * m
    #[serde(with = "crate::util::serde_integer")]
    nm: Integer,
}

pub struct Polynomial<'a> {
    sk: &'a PrivateKey,
    coefficients: Vec<Integer>,
}

pub fn generate_key_pair(
    bits: usize,
    decryption_servers: u32,
    threshold: u32,
) -> Result<(PublicKey, PrivateKey)> {
    let bits = bits / 2;
    let (mut t1, mut t2, mut t3, t4) = loop {
        let handle = thread::spawn(move || generate_safe_prime(bits));
        let (t3, t4) = generate_safe_prime(bits)?;
        let (t1, t2) = handle.join().expect("joining thread")?;
        if t1 != t3 {
            break (t1, t2, t3, t4);
        }
    };
    let n = t1.clone() * &t3;
    let n2 = n.clone().square();
    let g = n.clone() + 1;
    t3 = t2.clone() * t4;
    let nm = n.clone() * &t3;
    t1.assign(1);
    t2.assign(0);
    let d = util::crt2(&t1, &n, &t2, &t3);
    let delta = Integer::factorial(decryption_servers).complete();
    let mut combine_shares_constant = delta.clone().square();
    combine_shares_constant *= 4;
    if combine_shares_constant.invert_mut(&n).is_err() {
        return Err(anyhow!("No inverse"));
    }

    let pk = PublicKey {
        w: threshold,
        l: decryption_servers,
        n: n.clone(),
        g,
        n2: n2.clone(),
        delta,
        combine_shares_constant,
    };

    let sk = PrivateKey {
        w: threshold,
        l: decryption_servers,
        d,
        n,
        n2,
        nm,
    };

    Ok((pk, sk))
}

impl PrivateKeyShare {
    pub fn new(si: Integer, i: u32) -> Self {
        // i + 1 needed for zero indexed servers
        Self { i: i + 1, si }
    }
}

impl PublicKey {
    pub fn encrypt(&self, m: Integer, rand: &mut dyn MutRandState) -> Integer {
        // TODO is random_in_mult_group needed? Other implementations just choose 0 < r < n
        // https://crypto.stackexchange.com/questions/62371/paillier-encryption-problem-when-q-or-p-divides-r
        let mut r = random_in_mult_group(&self.n, rand);
        let mut rop = self.g.clone().pow_mod(&m, &self.n2).unwrap();
        r.pow_mod_mut(&self.n, &self.n2).unwrap();
        rop *= r;
        rop %= &self.n2;
        rop
    }

    pub fn reencrypt(&self, cipher: &mut Integer, rand: &mut dyn MutRandState) {
        let mut tmp = random_in_mult_group(&self.n, rand);
        tmp.pow_mod_mut(&self.n, &self.n2).unwrap();
        *cipher *= tmp;
        *cipher %= &self.n2;
    }

    pub fn add_plain(&self, cipher: &mut Integer, plain: &Integer) {
        let tmp = self.g.clone().pow_mod(plain, &self.n2).unwrap();
        *cipher *= tmp;
        *cipher %= &self.n2;
    }

    pub fn add_encrypted(&self, cipher1: &mut Integer, cipher2: &Integer) {
        *cipher1 *= cipher2;
        *cipher1 %= &self.n2;
    }

    pub fn mul_plain(&self, cipher: &mut Integer, plain: &Integer) {
        cipher.pow_mod_mut(plain, &self.n2).unwrap();
    }

    pub fn share_combine(&self, shares: &[PartialDecryption]) -> Result<Integer> {
        let mut cprime = Integer::from(1);
        for i in 0..self.w as usize {
            let mut lambda = self.delta.clone();
            for j in 0..self.w as usize {
                if i == j {
                    continue;
                }
                assert_ne!(
                    shares[i].id, shares[j].id,
                    "`share_combine` must be passed unique shares"
                );
                let v = Integer::from(shares[i].id as i64 - shares[j].id as i64);
                lambda = lambda * Integer::from(-(shares[j].id as i64)) / v;
            }
            let lambda2 = lambda.clone() * 2;
            cprime *= shares[i].val.clone().pow_mod(&lambda2, &self.n2).unwrap();
            cprime %= &self.n2;
        }
        let t = (cprime - 1) / &self.n;
        let rop = t * &self.combine_shares_constant % &self.n;
        Ok(rop)
    }
}

impl<'a> Polynomial<'a> {
    pub fn new<'b>(sk: &'a PrivateKey, rand: &'b mut dyn MutRandState) -> Self {
        let mut coefficients = vec![sk.nm.clone(); sk.w as usize];
        coefficients[0] = sk.d.clone();
        for coeff in coefficients.iter_mut().skip(1) {
            coeff.random_below_mut(rand);
        }
        Self { sk, coefficients }
    }

    pub fn compute(&self, x: u32) -> PrivateKeyShare {
        let mut rop = self.coefficients[0].clone();
        for (i, coeff) in self.coefficients.iter().enumerate().skip(1) {
            let mut tmp = Integer::u_pow_u(x + 1, i.try_into().unwrap()).complete();
            tmp *= coeff;
            rop += tmp;
            rop %= &self.sk.nm;
        }
        PrivateKeyShare::new(rop, x)
    }
}

impl PrivateKeyShare {
    pub fn share_decrypt(&self, pk: &PublicKey, cipher: Integer) -> PartialDecryption {
        let exponent = self.si.clone() * &pk.delta * 2;
        let share = cipher.pow_mod(&exponent, &pk.n2).unwrap();
        PartialDecryption {
            val: share,
            id: self.i,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::paillier::{generate_key_pair, Polynomial};

    use rug::rand::RandState;

    use rand::seq::SliceRandom;

    use rand::thread_rng;

    #[test]
    fn test_single_server() {
        let (pk, sk) = generate_key_pair(128, 1, 1).unwrap();
        let mut rand = RandState::new();
        let c = pk.encrypt(5.into(), &mut rand);
        let key_share = Polynomial::new(&sk, &mut rand).compute(0);
        let share_decrypt = key_share.share_decrypt(&pk, c);
        let combined = pk.share_combine(&[share_decrypt]).unwrap();
        assert_eq!(combined, 5);
    }

    #[test]
    fn test_multiple_server() {
        let (pk, sk) = generate_key_pair(128, 3, 3).unwrap();
        let mut rand = RandState::new();
        let c = pk.encrypt(10.into(), &mut rand);
        let poly = Polynomial::new(&sk, &mut rand);
        let key_shares: Vec<_> = (0..3).map(|idx| poly.compute(idx)).collect();

        let shares: Vec<_> = key_shares
            .iter()
            .map(|key_share| key_share.share_decrypt(&pk, c.clone()))
            .collect();
        let combined = pk.share_combine(&shares).unwrap();
        assert_eq!(combined, 10);
    }

    #[test]
    fn test_shuffled_shares() {
        let (pk, sk) = generate_key_pair(128, 3, 3).unwrap();
        let mut rand = RandState::new();
        let c = pk.encrypt(10.into(), &mut rand);
        let poly = Polynomial::new(&sk, &mut rand);
        let key_shares: Vec<_> = (0..3).map(|idx| poly.compute(idx)).collect();
        let mut shares: Vec<_> = key_shares
            .iter()
            .map(|key_share| key_share.share_decrypt(&pk, c.clone()))
            .collect();
        shares.shuffle(&mut thread_rng());
        let combined = pk.share_combine(&shares).unwrap();
        assert_eq!(combined, 10);
    }

    #[test]
    fn test_multiple_server_lower_threshold() {
        let (pk, sk) = generate_key_pair(128, 3, 2).unwrap();
        let mut rand = RandState::new();
        let c = pk.encrypt(10.into(), &mut rand);
        let poly = Polynomial::new(&sk, &mut rand);
        let key_shares: Vec<_> = (0..2).map(|idx| poly.compute(idx)).collect();

        let shares: Vec<_> = key_shares
            .iter()
            .map(|key_share| key_share.share_decrypt(&pk, c.clone()))
            .collect();
        let combined = pk.share_combine(&shares).unwrap();
        assert_eq!(combined, 10);
    }
}
