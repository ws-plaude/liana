# v15 descriptor backups

Exported from liana-gui v15.0 (`15.x`, `fa0bdf7bf`) with `Settings > ImportExport`, one wallet per network:

- `v15-<network>-descriptor.txt`: "Descriptor only - plain-text"
- `v15-<network>-descriptor.bed`: "Encrypted descriptor"

Policy: `tr(uncover, and_v(v:pk(hotel), older(65535)))`. Test keys only, the mnemonics are public, never use them for real funds.

The 4 test networks share the same descriptor (`#kvsc70ze`), mainnet is `#eukwu52c`.

## Seeds

| key | fingerprint | mnemonic |
|---|---|---|
| uncover (primary) | `f25bdff6` | `uncover library ride voyage nerve hunt share expire wisdom mule omit hint` |
| hotel (recovery) | `6abb52a9` | `hotel organ vacuum praise bacon gentle love another absurd crystal cloud window` |

## Xpubs (to decrypt the .bed files, either key works)

Mainnet, `48'/0'/0'/2'`:

- uncover: `[f25bdff6/48'/0'/0'/2']xpub6E9n8JKbzYeQRecVnWTz2MFmH6NdsogPNiGHz6yYC4Y7R16qm58FxBjXqFQsrPu3Vw4wsucfTPy9AsUHbCnW6NY3EvaAYvdAdofQDggw1CR`
- hotel: `[6abb52a9/48'/0'/0'/2']xpub6EeNdrL8utQvNMvddSaFf3xXo31r9WjN7LiKDozVv1VYBNuT1ZmT2pkRgguWCZ9UZvsTzYdqDrk9Ag2vEQm9VzYxsv2G1guzhW1bjBvs3mL`

Testnet, testnet4, signet, regtest, `48'/1'/0'/2'`:

- uncover: `[f25bdff6/48'/1'/0'/2']tpubDF8cqMgmJ6BMJwMoEhwAfgVDdXs29y6w2qG1i1ciaYVmqQ6cRTjNoqWJZD2kAR6vJrGcpVBVyYEgYm5GE88F3Z2SVbQxqwdbRZeyUeGwTnk`
- hotel: `[6abb52a9/48'/1'/0'/2']tpubDFZTCVU1Sa9nJXCxx97UFvGausHQPFjJyaiDbdr8GNqjCLKwYc8ihegK7yJdcizs9HMbiGA7ke1HiCENVHaERvNANHW7U2Wo2qnRsuqB52r`
