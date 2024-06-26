use std::sync::Arc;

use anyhow::{anyhow, ensure};
use snarkvm::{
    algorithms::{
        fft::{domain::FFTPrecomputation, EvaluationDomain},
        polycommit::kzg10::{UniversalParams, VerifierKey},
    },
    curves::PairingEngine,
    prelude::{Environment, Network, Result},
};

pub type CoinbaseVerifyingKey<N> = VerifierKey<<N as Environment>::PairingCurve>;

#[derive(Clone, Debug)]
struct CoinbaseProvingKey<N: Network> {
    /// The key used to commit to polynomials in Lagrange basis.
    pub lagrange_basis_at_beta_g: Vec<<N::PairingCurve as PairingEngine>::G1Affine>,
    /// Domain used to compute the product of the epoch polynomial and the prover polynomial.
    pub product_domain: EvaluationDomain<<N::PairingCurve as PairingEngine>::Fr>,
    /// Precomputation to speed up FFTs.
    pub fft_precomputation: FFTPrecomputation<<N::PairingCurve as PairingEngine>::Fr>,
    /// Elements of the product domain.
    pub product_domain_elements: Vec<<N::PairingCurve as PairingEngine>::Fr>,
    /// The verifying key of the coinbase puzzle.
    pub verifying_key: CoinbaseVerifyingKey<N>,
}
#[derive(Clone)]
pub enum CoinbasePuzzle<N: Network> {
    /// The prover contains the coinbase puzzle proving key.
    Prover(Arc<CoinbaseProvingKey<N>>),
    /// The verifier contains the coinbase puzzle verifying key.
    Verifier(Arc<CoinbaseVerifyingKey<N>>),
}

impl<N: Network> CoinbasePuzzle<N> {
    pub fn new(srs: &UniversalParams<N::PairingCurve>, degree: u32) -> Result<Self> {
        // As above, we must support committing to the product of two degree `n` polynomials.
        // Thus, the SRS must support committing to a polynomial of degree `2n - 1`.
        // Since the upper bound to `srs.powers_of_beta_g` takes as input the number
        // of coefficients. The degree of the product has `2n - 1` coefficients.
        //
        // Hence, we request the powers of beta for the interval [0, 2n].
        let product_domain = Self::product_domain(degree)?;

        let lagrange_basis_at_beta_g = srs.lagrange_basis(product_domain)?;
        let fft_precomputation = product_domain.precompute_fft();
        let product_domain_elements = product_domain.elements().collect();

        let vk = CoinbaseVerifyingKey::<N> {
            g: srs.power_of_beta_g(0)?,
            gamma_g: <N::PairingCurve as PairingEngine>::G1Affine::zero(), /* We don't use gamma_g later on since we are not hiding. */
            h: srs.h,
            beta_h: srs.beta_h(),
            prepared_h: srs.prepared_h.clone(),
            prepared_beta_h: srs.prepared_beta_h.clone(),
        };

        let pk = CoinbaseProvingKey {
            product_domain,
            product_domain_elements,
            lagrange_basis_at_beta_g,
            fft_precomputation,
            verifying_key: vk,
        };

        Ok(Self::Prover(Arc::new(pk)))
    }

    pub(crate) fn product_domain(degree: u32) -> Result<EvaluationDomain<N::Field>> {
        ensure!(degree != 0, "Degree cannot be zero");
        let num_coefficients = degree.checked_add(1).ok_or_else(|| anyhow!("Degree is too large"))?;
        let product_num_coefficients = num_coefficients
            .checked_mul(2)
            .and_then(|t| t.checked_sub(1))
            .ok_or_else(|| anyhow!("Degree is too large"))?;
        assert_eq!(product_num_coefficients, 2 * degree + 1);
        let product_domain =
            EvaluationDomain::new(product_num_coefficients.try_into()?).ok_or_else(|| anyhow!("Invalid degree"))?;
        assert_eq!(
            product_domain.size(),
            (product_num_coefficients as usize).checked_next_power_of_two().unwrap()
        );
        Ok(product_domain)
    }

    /// Returns the coinbase verifying key.
    pub fn verifying_key(&self) -> &CoinbaseVerifyingKey<N> {
        match self {
            Self::Prover(coinbase_proving_key) => &coinbase_proving_key.verifying_key,
            Self::Verifier(coinbase_verifying_key) => coinbase_verifying_key,
        }
    }
}
