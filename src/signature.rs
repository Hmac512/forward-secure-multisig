use rand::{CryptoRng, RngCore};

use amcl_wrapper::field_elem::{FieldElement, FieldElementVector};
use amcl_wrapper::group_elem::{GroupElement, GroupElementVector};

use crate::errors::ForwardSecureSignatureError;
use crate::keys::{Sigkey, Verkey};
use crate::util::{calculate_path_factor_using_t_l, from_node_num_to_path, GeneratorSet};
use crate::{ate_multi_pairing, SignatureGroup, SignatureGroupVec, VerkeyGroup};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub sigma_1: SignatureGroup,
    pub sigma_2: VerkeyGroup,
}

impl Signature {
    /// Creates new in-deterministic signature
    pub fn new<R: RngCore + CryptoRng>(
        msg: &[u8],
        t: u128,
        l: u8,
        gens: &GeneratorSet,
        sig_key: &Sigkey,
        rng: &mut R,
    ) -> Result<Self, ForwardSecureSignatureError> {
        if gens.1.len() < (l as usize + 2) {
            return Err(ForwardSecureSignatureError::NotEnoughGenerators { n: l as usize + 2 });
        }

        let r = FieldElement::random_using_rng(rng);
        Self::gen_sig(msg, t, l, gens, sig_key, r)
    }

    /// Creates new deterministic signature. Signature for same message and secret key will be equal
    pub fn new_deterministic(
        msg: &[u8],
        t: u128,
        l: u8,
        gens: &GeneratorSet,
        sig_key: &Sigkey,
    ) -> Result<Self, ForwardSecureSignatureError> {
        if gens.1.len() < (l as usize + 2) {
            return Err(ForwardSecureSignatureError::NotEnoughGenerators { n: l as usize + 2 });
        }
        let r = Self::gen_sig_rand(msg, t, sig_key);
        Self::gen_sig(msg, t, l, gens, sig_key, r)
    }

    pub fn aggregate(sigs: Vec<&Self>) -> Self {
        let mut asig_1 = SignatureGroup::identity();
        let mut asig_2 = VerkeyGroup::identity();
        for s in sigs {
            asig_1 += &s.sigma_1;
            asig_2 += &s.sigma_2;
        }
        Self {
            sigma_1: asig_1,
            sigma_2: asig_2,
        }
    }

    pub fn verify(
        &self,
        msg: &[u8],
        t: u128,
        l: u8,
        gens: &GeneratorSet,
        verkey: &Verkey,
    ) -> Result<bool, ForwardSecureSignatureError> {
        if gens.1.len() < (l as usize + 2) {
            return Err(ForwardSecureSignatureError::NotEnoughGenerators { n: l as usize + 2 });
        }

        if self.is_identity() || verkey.is_identity() || !self.has_correct_oder() {
            return Ok(false);
        }
        Self::verify_naked(&self.sigma_1, &self.sigma_2, &verkey.value, msg, t, l, gens)
    }

    pub fn verify_aggregated(
        &self,
        msg: &[u8],
        t: u128,
        l: u8,
        ver_keys: Vec<&Verkey>,
        gens: &GeneratorSet,
    ) -> Result<bool, ForwardSecureSignatureError> {
        let avk = Verkey::aggregate(ver_keys);
        self.verify(msg, t, l, gens, &avk)
    }

    /// Hash message in the field before signing or verification
    fn hash_message(message: &[u8]) -> FieldElement {
        // Fixme: This is not accurate and might affect the security proof but should work in practice
        FieldElement::from_msg_hash(message)
    }

    /// Generate random number for signature using message time period and signing key for that time period.
    fn gen_sig_rand(message: &[u8], t: u128, sig_key: &Sigkey) -> FieldElement {
        let mut bytes = message.to_vec();
        bytes.extend_from_slice(&sig_key.0.to_bytes());
        for i in &sig_key.1 {
            bytes.extend_from_slice(&i.to_bytes());
        }
        bytes.extend_from_slice(&t.to_le_bytes());
        FieldElement::from_msg_hash(&bytes)
    }

    fn gen_sig(
        msg: &[u8],
        t: u128,
        l: u8,
        gens: &GeneratorSet,
        sig_key: &Sigkey,
        r: FieldElement,
    ) -> Result<Self, ForwardSecureSignatureError> {
        let c = sig_key.0.clone();
        let d = sig_key.1[0].clone();

        // Hash(msg) -> FieldElement
        let m = Self::hash_message(msg);

        let sigma_2 = &c + (&gens.0 * &r);

        // e_l
        let e_l = sig_key.1[sig_key.1.len() - 1].clone();
        let pf = calculate_path_factor_using_t_l(t, l, gens)?;

        // sigma_1 = d + (e_l * &m) + (pf + (gens.1[l as usize + 1] * m))*r
        let mut sigma_1 = d;
        let mut points = SignatureGroupVec::with_capacity(3);
        let mut scalars = FieldElementVector::with_capacity(3);

        // (e_l * &m)
        points.push(e_l);
        scalars.push(m.clone());

        // gens.1[l as usize + 1] * (m * r)
        points.push(gens.1[l as usize + 1].clone());
        scalars.push(m * &r);

        // pf * r
        points.push(pf);
        scalars.push(r);

        sigma_1 += points
            .multi_scalar_mul_const_time(scalars.as_ref())
            .unwrap();

        Ok(Self {
            sigma_1: sigma_1.clone(),
            sigma_2: sigma_2.clone(),
        })
    }

    fn verify_naked(
        sigma_1: &SignatureGroup,
        sigma_2: &VerkeyGroup,
        verkey: &VerkeyGroup,
        msg: &[u8],
        t: u128,
        l: u8,
        gens: &GeneratorSet,
    ) -> Result<bool, ForwardSecureSignatureError> {
        let h = &gens.1[0];
        let g2 = &gens.0;
        let y = verkey;
        let m = Self::hash_message(msg);
        let mut sigma_1_1 = calculate_path_factor_using_t_l(t, l, gens)?;
        sigma_1_1 += &gens.1[l as usize + 1] * m;

        // Check that e(sigma_1, g2) == e(h, y) * e(sigma_1_1, sigma_2)
        // This is equivalent to checking e(h, y) * e(sigma_1_1, sigma_2) * e(sigma_1, g2)^-1 == 1
        // Which comes out to be e(h, y) * e(sigma_1_1, sigma_2) * e(sigma_1, -g2) == 1 which can put in a multi-pairing.
        // -g2 can be precomputed if performance is critical
        // Similarly it might be better to precompute e(h, y) and do a 2-pairing than a 3-pairing
        let e = ate_multi_pairing(vec![
            (&sigma_1, &g2.negation()),
            (h, y),
            (&sigma_1_1, sigma_2),
        ]);
        Ok(e.is_one())
    }

    fn is_identity(&self) -> bool {
        if self.sigma_1.is_identity() {
            println!("Signature point in G1 at infinity");
            return true;
        }
        if self.sigma_2.is_identity() {
            println!("Signature point in G2 at infinity");
            return true;
        }
        return false;
    }

    fn has_correct_oder(&self) -> bool {
        if !self.sigma_1.has_correct_order() {
            println!("Signature point in G1 has incorrect order");
            return false;
        }
        if !self.sigma_2.has_correct_order() {
            println!("Signature point in G2 has incorrect order");
            return false;
        }
        return true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{setup, InMemorySigKeyDatabase, Keypair, SigKeyDb, SigManager};
    use crate::util::calculate_l;
    use rand::rngs::ThreadRng;
    // For benchmarking
    use std::time::{Duration, Instant};

    pub fn create_sig_and_verify<R: RngCore + CryptoRng>(
        set: &SigManager,
        t: u128,
        vk: &Verkey,
        l: u8,
        gens: &GeneratorSet,
        mut rng: &mut R,
        db: &dyn SigKeyDb,
    ) {
        let sk = SigManager::get_key(t, db).unwrap();
        let msg = "Hello".as_bytes();
        let sig = Signature::new(msg, t, l, &gens, &sk, &mut rng).unwrap();
        assert!(sig.verify(msg, t, l, &gens, &vk).unwrap());
    }

    fn fast_forward_sig_and_verify<R: RngCore + CryptoRng>(
        set: &mut SigManager,
        t: u128,
        vk: &Verkey,
        l: u8,
        gens: &GeneratorSet,
        mut rng: &mut R,
        db: &mut dyn SigKeyDb,
    ) {
        set.fast_forward_update(t, &gens, &mut rng, db).unwrap();
        create_sig_and_verify(&set, t, &vk, l, &gens, &mut rng, db);
    }

    #[test]
    fn test_sig_verify_initial() {
        let mut rng = rand::thread_rng();
        let T = 7;
        let l = calculate_l(T).unwrap();
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, set, _) = setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();
        let t = 1u128;
        create_sig_and_verify::<ThreadRng>(&set, t, &vk, l, &gens, &mut rng, &db);
    }

    #[test]
    fn test_sig_deterministic() {
        let mut rng = rand::thread_rng();
        let T = 7;
        let l = calculate_l(T).unwrap();
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, mut set, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();
        let t1 = 1u128;
        let msg = "Hello".as_bytes();
        let sk1 = SigManager::get_key(t1, &db).unwrap();

        // In-deterministic and deterministic signatures for t=1

        let sig1 = Signature::new(msg, t1, l, &gens, &sk1, &mut rng).unwrap();
        let sig1_deterministic = Signature::new_deterministic(msg, t1, l, &gens, &sk1).unwrap();
        // In-deterministic sigs should verify
        assert!(sig1.verify(msg, t1, l, &gens, &vk).unwrap());
        assert!(sig1_deterministic.verify(msg, t1, l, &gens, &vk).unwrap());

        let sig2 = Signature::new(msg, t1, l, &gens, &sk1, &mut rng).unwrap();
        let sig2_deterministic = Signature::new_deterministic(msg, t1, l, &gens, &sk1).unwrap();
        // Deterministic sigs should verify
        assert!(sig2.verify(msg, t1, l, &gens, &vk).unwrap());
        assert!(sig2_deterministic.verify(msg, t1, l, &gens, &vk).unwrap());

        // Deterministic sigs for same message and secret key should be equal
        assert_eq!(sig1_deterministic, sig2_deterministic);
        // In-deterministic sigs for same message and secret key should be different
        assert_ne!(sig1, sig2);

        // In-deterministic and deterministic signatures for t=2, doing the same checks as above
        let t2 = 2u128;
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        let sk2 = SigManager::get_key(t2, &db).unwrap();

        let sig3 = Signature::new(msg, t2, l, &gens, &sk2, &mut rng).unwrap();
        let sig3_deterministic = Signature::new_deterministic(msg, t2, l, &gens, &sk2).unwrap();
        assert!(sig3.verify(msg, t2, l, &gens, &vk).unwrap());
        assert!(sig3_deterministic.verify(msg, t2, l, &gens, &vk).unwrap());

        let sig4 = Signature::new(msg, t2, l, &gens, &sk2, &mut rng).unwrap();
        let sig4_deterministic = Signature::new_deterministic(msg, t2, l, &gens, &sk2).unwrap();
        assert!(sig4.verify(msg, t2, l, &gens, &vk).unwrap());
        assert!(sig4_deterministic.verify(msg, t2, l, &gens, &vk).unwrap());

        assert_eq!(sig3_deterministic, sig4_deterministic);
        assert_ne!(sig3, sig4);

        // deterministic signatures for different secret keys should differ
        assert_ne!(sig1_deterministic, sig3_deterministic);
    }

    #[test]
    fn test_sig_verify_post_simple_update_by_7() {
        let mut rng = rand::thread_rng();
        let T = 7;
        let l = calculate_l(T).unwrap();
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, mut set, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

        // t=2
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 2u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 5u128, &vk, l, &gens, &mut rng, &db);

        // t=3
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 3u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 4u128, &vk, l, &gens, &mut rng, &db);

        // t=4
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 4u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 5u128, &vk, l, &gens, &mut rng, &db);

        // t=5
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 5u128, &vk, l, &gens, &mut rng, &db);

        // t=6
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 6u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 7u128, &vk, l, &gens, &mut rng, &db);

        // t=7
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 7u128, &vk, l, &gens, &mut rng, &db);
    }

    #[test]
    fn test_sig_verify_post_simple_update_by_15() {
        let mut rng = rand::thread_rng();
        let T = 15;
        let l = calculate_l(T).unwrap();
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, mut set, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

        // t=2
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 2u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=3
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 3u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 6u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=4
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 4u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 5u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 6u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=5
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 5u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 6u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=6
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 6u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=7
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 7u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 8u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=8
        set.simple_update(&gens, &mut rng, &mut db).unwrap();

        // t=9
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 9u128, &vk, l, &gens, &mut rng, &db);

        // t=10
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 10u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 13u128, &vk, l, &gens, &mut rng, &db);

        // t=11
        set.simple_update(&gens, &mut rng, &mut db).unwrap();
        create_sig_and_verify::<ThreadRng>(&set, 11u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 12u128, &vk, l, &gens, &mut rng, &db);
        create_sig_and_verify::<ThreadRng>(&set, 13u128, &vk, l, &gens, &mut rng, &db);
    }

    #[test]
    fn test_sig_verify_post_fast_forward_update_7() {
        let mut rng = rand::thread_rng();
        let T = 7;
        let l = calculate_l(T).unwrap();
        let mut t = 1u128;

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 3;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 4;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 5;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 6;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 7;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }
    }

    #[test]
    fn test_sig_verify_post_fast_forward_update_repeat_7() {
        let mut rng = rand::thread_rng();
        let T = 7;
        let l = calculate_l(T).unwrap();
        let mut t = 1u128;
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, mut set, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

        t = 2;
        fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

        t = 4;
        fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

        t = 6;
        fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
    }

    #[test]
    fn test_sig_verify_post_fast_forward_update_repeat_15() {
        let mut rng = rand::thread_rng();
        let T = 15;
        let l = calculate_l(T).unwrap();
        let mut t = 1u128;

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 3;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 8;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 13;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 6;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 8;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 13;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }
    }

    #[test]
    fn test_sig_verify_post_fast_forward_update_repeat_65535() {
        let mut rng = rand::thread_rng();
        let T = 65535;
        let l = calculate_l(T).unwrap();
        let mut t = 1u128;

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 4;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 15;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 16;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 32;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 1024;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 4095;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 65535;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }

        {
            let mut db = InMemorySigKeyDatabase::new();
            let (gens, vk, mut set, _) =
                setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

            t = 15;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 1023;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 8191;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);

            t = 16384;
            fast_forward_sig_and_verify(&mut set, t, &vk, l, &gens, &mut rng, &mut db);
        }
    }

    #[test]
    fn test_aggr_sig_verify() {
        let mut rng = rand::thread_rng();
        let T = 7;
        let l = calculate_l(T).unwrap();
        let mut t = 1u128;

        let mut db1 = InMemorySigKeyDatabase::new();
        let (gens, vk1, mut sigkey_set1, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db1).unwrap();

        let mut db2 = InMemorySigKeyDatabase::new();
        let (keypair2, mut sigkey_set2) = Keypair::new(T, &gens, &mut rng, &mut db2).unwrap();
        let vk2 = keypair2.ver_key;

        create_sig_and_verify::<ThreadRng>(&sigkey_set1, t, &vk1, l, &gens, &mut rng, &db1);
        create_sig_and_verify::<ThreadRng>(&sigkey_set2, t, &vk2, l, &gens, &mut rng, &db2);

        {
            let msg = "Hello".as_bytes();
            let sk1 = SigManager::get_key(t, &db1).unwrap();
            let sig1 = Signature::new(msg, t, l, &gens, &sk1, &mut rng).unwrap();
            let sk2 = SigManager::get_key(t, &db2).unwrap();
            let sig2 = Signature::new(msg, t, l, &gens, &sk2, &mut rng).unwrap();

            let asig = Signature::aggregate(vec![&sig1, &sig2]);
            assert!(asig
                .verify_aggregated(msg, t, l, vec![&vk1, &vk2], &gens)
                .unwrap());
        }

        {
            t = 3;
            sigkey_set1
                .fast_forward_update(t, &gens, &mut rng, &mut db1)
                .unwrap();
            sigkey_set2
                .fast_forward_update(t, &gens, &mut rng, &mut db2)
                .unwrap();

            let msg = "Hello".as_bytes();
            let sk1 = SigManager::get_key(t, &db1).unwrap();
            let sig1 = Signature::new(msg, t, l, &gens, &sk1, &mut rng).unwrap();
            let sk2 = SigManager::get_key(t, &db2).unwrap();
            let sig2 = Signature::new(msg, t, l, &gens, &sk2, &mut rng).unwrap();

            let asig = Signature::aggregate(vec![&sig1, &sig2]);
            assert!(asig
                .verify_aggregated(msg, t, l, vec![&vk1, &vk2], &gens)
                .unwrap());
        }

        {
            t = 5;
            sigkey_set1
                .fast_forward_update(t, &gens, &mut rng, &mut db1)
                .unwrap();
            sigkey_set2
                .fast_forward_update(t, &gens, &mut rng, &mut db2)
                .unwrap();

            let msg = "Hello".as_bytes();
            let sk1 = SigManager::get_key(t, &db1).unwrap();
            let sig1 = Signature::new(msg, t, l, &gens, &sk1, &mut rng).unwrap();
            let sk2 = SigManager::get_key(t, &db2).unwrap();
            let sig2 = Signature::new(msg, t, l, &gens, &sk2, &mut rng).unwrap();

            let asig = Signature::aggregate(vec![&sig1, &sig2]);
            assert!(asig
                .verify_aggregated(msg, t, l, vec![&vk1, &vk2], &gens)
                .unwrap());
        }
    }

    #[test]
    fn timing_sig_verify_post_update_65535() {
        // For tree with l=16, supports 2^16 - 1 = 65535 keys
        // Benchmarking signature time with only simple update as fast forward update does not matter for signing.
        let mut rng = rand::thread_rng();
        let T = 65535;
        let l = calculate_l(T).unwrap();
        let msg = "Hello".as_bytes();
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, mut set, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

        for t in 2..20 {
            set.simple_update(&gens, &mut rng, &mut db).unwrap();
            let sk = SigManager::get_key(t, &db).unwrap();
            let start = Instant::now();
            let sig = Signature::new(msg, t, l, &gens, &sk, &mut rng).unwrap();
            println!(
                "For l={}, time to sign for t={} is {:?}",
                l,
                t,
                start.elapsed()
            );
            let start = Instant::now();
            assert!(sig.verify(msg, t, l, &gens, &vk).unwrap());
            println!(
                "For l={}, time to verify for t={} is {:?}",
                l,
                t,
                start.elapsed()
            );
        }
    }

    #[test]
    fn timing_sig_verify_post_update_1048575() {
        // For tree with l=20, supports 2^20 - 1 = 1048575 keys
        // Benchmarking signature time with only simple update as fast forward update does not matter for signing.

        let mut rng = rand::thread_rng();
        let T = 1048575;
        let l = calculate_l(T).unwrap();
        let msg = "Hello".as_bytes();
        let mut db = InMemorySigKeyDatabase::new();
        let (gens, vk, mut set, _) =
            setup::<ThreadRng>(T, "test_pixel", &mut rng, &mut db).unwrap();

        for t in 2..30 {
            set.simple_update(&gens, &mut rng, &mut db).unwrap();
            let sk = SigManager::get_key(t, &db).unwrap();
            let start = Instant::now();
            let sig = Signature::new(msg, t, l, &gens, &sk, &mut rng).unwrap();
            println!(
                "For l={}, time to sign for t={} is {:?}",
                l,
                t,
                start.elapsed()
            );
            let start = Instant::now();
            assert!(sig.verify(msg, t, l, &gens, &vk).unwrap());
            println!(
                "For l={}, time to verify for t={} is {:?}",
                l,
                t,
                start.elapsed()
            );
        }
    }
}
