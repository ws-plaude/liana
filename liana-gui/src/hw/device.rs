use std::{future::Future, sync::Arc};

use bwk_hwi::{AddressScript, DeviceKind, Error, Version, HWI};
use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint, Xpub},
    Psbt,
};

/// Run a hardware wallet call off the async runtime.
///
/// The only place where the hop from async to blocking code happens: every device call made from
/// async code goes through it. A blocking task cannot be cancelled, so dropping the returned
/// future does not stop the device call, which runs until the device answers or the user acts on
/// it.
pub(super) fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Result<T, Error>> {
    let task = tokio::task::spawn_blocking(f);
    async move {
        task.await
            .map_err(|e| Error::Device(format!("hardware wallet task failed: {e}")))
    }
}

/// A hardware wallet driven from async code.
///
/// The device methods block until the device answers, often waiting for the user to press a
/// button, so each one runs on a blocking task. See [`run_blocking`] for the cancellation caveat.
#[derive(Debug, Clone)]
pub struct AsyncDevice(Arc<dyn HWI + Send + Sync>);

impl AsyncDevice {
    pub fn new(device: Arc<dyn HWI + Send + Sync>) -> Self {
        Self(device)
    }

    pub fn device_kind(&self) -> DeviceKind {
        self.0.device_kind()
    }

    pub async fn get_version(&self) -> Result<Version, Error> {
        let device = self.0.clone();
        run_blocking(move || device.get_version()).await?
    }

    pub async fn get_master_fingerprint(&self) -> Result<Fingerprint, Error> {
        let device = self.0.clone();
        run_blocking(move || device.get_master_fingerprint()).await?
    }

    pub async fn get_extended_pubkey(&self, path: &DerivationPath) -> Result<Xpub, Error> {
        let device = self.0.clone();
        let path = path.clone();
        run_blocking(move || device.get_extended_pubkey(&path)).await?
    }

    pub async fn register_wallet(
        &self,
        name: &str,
        policy: &str,
    ) -> Result<Option<[u8; 32]>, Error> {
        let device = self.0.clone();
        let (name, policy) = (name.to_string(), policy.to_string());
        run_blocking(move || device.register_wallet(&name, &policy)).await?
    }

    pub async fn is_wallet_registered(&self, name: &str, policy: &str) -> Result<bool, Error> {
        let device = self.0.clone();
        let (name, policy) = (name.to_string(), policy.to_string());
        run_blocking(move || device.is_wallet_registered(&name, &policy)).await?
    }

    pub async fn display_address(&self, script: &AddressScript) -> Result<(), Error> {
        let device = self.0.clone();
        let script = script.clone();
        run_blocking(move || device.display_address(&script)).await?
    }

    /// The signed psbt is written back into `psbt` only if the device signed it.
    pub async fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), Error> {
        let device = self.0.clone();
        let mut signing = psbt.clone();
        *psbt = run_blocking(move || device.sign_tx(&mut signing).map(|_| signing)).await??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use liana::miniscript::bitcoin::{
        absolute::LockTime, transaction::Version as TxVersion, Amount, ScriptBuf, Transaction,
        TxOut,
    };

    use super::*;

    const XPUB: &str = "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8";
    const FINGERPRINT: &str = "f5acc2fd";
    const HMAC: [u8; 32] = [7; 32];
    /// Output `sign_tx` adds to the transaction, standing in for a signature.
    const SIGNED_MARKER: Amount = Amount::from_sat(4_200);

    /// Answers every call with canned data, or with the error the test picked.
    #[derive(Debug)]
    struct FakeDevice {
        error: Option<Error>,
    }

    impl FakeDevice {
        fn check(&self) -> Result<(), Error> {
            match &self.error {
                Some(e) => Err(e.clone()),
                None => Ok(()),
            }
        }
    }

    impl HWI for FakeDevice {
        fn device_kind(&self) -> DeviceKind {
            DeviceKind::Specter
        }

        fn get_version(&self) -> Result<Version, Error> {
            self.check()?;
            Ok(Version {
                major: 1,
                minor: 2,
                patch: 3,
                prerelease: None,
            })
        }

        fn get_master_fingerprint(&self) -> Result<Fingerprint, Error> {
            self.check()?;
            Fingerprint::from_str(FINGERPRINT).map_err(|e| Error::Device(e.to_string()))
        }

        fn get_extended_pubkey(&self, _path: &DerivationPath) -> Result<Xpub, Error> {
            self.check()?;
            Xpub::from_str(XPUB).map_err(|e| Error::Device(e.to_string()))
        }

        fn register_wallet(&self, _name: &str, _policy: &str) -> Result<Option<[u8; 32]>, Error> {
            self.check()?;
            Ok(Some(HMAC))
        }

        fn is_wallet_registered(&self, _name: &str, _policy: &str) -> Result<bool, Error> {
            self.check()?;
            Ok(true)
        }

        fn display_address(&self, _script: &AddressScript) -> Result<(), Error> {
            self.check()
        }

        fn sign_tx(&self, psbt: &mut Psbt) -> Result<(), Error> {
            self.check()?;
            psbt.unsigned_tx.output.push(TxOut {
                value: SIGNED_MARKER,
                script_pubkey: ScriptBuf::new(),
            });
            psbt.outputs.push(Default::default());
            Ok(())
        }
    }

    fn device(error: Option<Error>) -> AsyncDevice {
        AsyncDevice::new(Arc::new(FakeDevice { error }))
    }

    fn address() -> AddressScript {
        AddressScript::Miniscript {
            index: 0,
            change: false,
        }
    }

    fn unsigned_psbt() -> Psbt {
        Psbt::from_unsigned_tx(Transaction {
            version: TxVersion::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        })
        .expect("no input to sign")
    }

    #[tokio::test]
    async fn device_answers_are_returned_unchanged() {
        let hw = device(None);
        assert_eq!(hw.device_kind(), DeviceKind::Specter);
        assert_eq!(
            hw.get_version().await.expect("no error").to_string(),
            "1.2.3"
        );
        assert_eq!(
            hw.get_master_fingerprint()
                .await
                .expect("no error")
                .to_string(),
            FINGERPRINT
        );
        assert_eq!(
            hw.get_extended_pubkey(&DerivationPath::master())
                .await
                .expect("no error")
                .to_string(),
            XPUB
        );
        assert_eq!(
            hw.register_wallet("liana", "wsh(pk(A))")
                .await
                .expect("no error"),
            Some(HMAC)
        );
        assert!(hw
            .is_wallet_registered("liana", "wsh(pk(A))")
            .await
            .expect("no error"));
        hw.display_address(&address()).await.expect("no error");
    }

    #[tokio::test]
    async fn device_error_is_propagated_unchanged() {
        let hw = device(Some(Error::UserRefused));
        assert!(matches!(
            hw.get_version().await.expect_err("device refuses"),
            Error::UserRefused
        ));
        assert!(matches!(
            hw.display_address(&address())
                .await
                .expect_err("device refuses"),
            Error::UserRefused
        ));

        let mut psbt = unsigned_psbt();
        assert!(matches!(
            hw.sign_tx(&mut psbt).await.expect_err("device refuses"),
            Error::UserRefused
        ));
        assert!(psbt.unsigned_tx.output.is_empty());
    }

    #[tokio::test]
    async fn sign_tx_writes_the_signed_psbt_back() {
        let hw = device(None);
        let mut psbt = unsigned_psbt();
        hw.sign_tx(&mut psbt).await.expect("no error");
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(psbt.unsigned_tx.output[0].value, SIGNED_MARKER);
        assert_eq!(psbt.outputs.len(), 1);
    }
}
