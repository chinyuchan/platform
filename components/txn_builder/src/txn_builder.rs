#![deny(warnings)]
extern crate ledger;
extern crate serde;
extern crate zei;
#[macro_use]
extern crate serde_derive;

use ledger::data_model::errors::PlatformError;
use ledger::data_model::*;
use rand_chacha::ChaChaRng;
use rand_core::SeedableRng;
use std::collections::HashSet;
use zei::serialization::ZeiFromToBytes;
use zei::setup::PublicParams;
use zei::xfr::asset_record::{build_blind_asset_record, open_asset_record};
use zei::xfr::sig::{XfrKeyPair, XfrPublicKey, XfrSecretKey};
use zei::xfr::structs::{AssetIssuerPubKeys, AssetRecord, BlindAssetRecord, OpenAssetRecord};

pub trait BuildsTransactions {
  fn transaction(&self) -> &Transaction;
  #[allow(clippy::too_many_arguments)]
  fn add_operation_create_asset(&mut self,
                                pub_key: &IssuerPublicKey,
                                priv_key: &XfrSecretKey,
                                token_code: Option<AssetTypeCode>,
                                updatable: bool,
                                traceable: bool,
                                memo: &str)
                                -> Result<&mut Self, PlatformError>;
  fn add_operation_issue_asset(&mut self,
                               pub_key: &IssuerPublicKey,
                               priv_key: &XfrSecretKey,
                               token_code: &AssetTypeCode,
                               seq_num: u64,
                               records: &[TxOutput])
                               -> Result<&mut Self, PlatformError>;
  fn add_operation_transfer_asset(&mut self,
                                  input_sids: Vec<TxoRef>,
                                  input_records: &[OpenAssetRecord],
                                  output_records: &[AssetRecord])
                                  -> Result<&mut Self, PlatformError>;
  fn serialize(&self) -> Result<Vec<u8>, PlatformError>;
  fn serialize_str(&self) -> Result<String, PlatformError>;

  fn add_operation(&mut self, op: Operation) -> &mut Self;

  fn add_basic_issue_asset(&mut self,
                           pub_key: &IssuerPublicKey,
                           priv_key: &XfrSecretKey,
                           tracking_keys: &Option<AssetIssuerPubKeys>,
                           token_code: &AssetTypeCode,
                           seq_num: u64,
                           amount: u64)
                           -> Result<&mut Self, PlatformError> {
    let mut prng = ChaChaRng::from_seed([0u8; 32]);
    let params = PublicParams::new();
    let ar = AssetRecord::new(amount, token_code.val, pub_key.key)?;
    let ba = build_blind_asset_record(&mut prng, &params.pc_gens, &ar, true, true, tracking_keys);
    self.add_operation_issue_asset(pub_key, priv_key, token_code, seq_num, &[TxOutput(ba)])?;
    Ok(self)
  }

  #[allow(clippy::comparison_chain)]
  fn add_basic_transfer_asset(&mut self,
                              key_pair: &XfrKeyPair,
                              transfer_from: &[(&TxoRef, &BlindAssetRecord, u64)],
                              transfer_to: &[(u64, &AccountAddress)])
                              -> Result<&mut Self, PlatformError> {
    let input_sids: Vec<TxoRef> = transfer_from.iter()
                                               .map(|(ref txo_sid, _, _)| *(*txo_sid))
                                               .collect();
    let input_amounts: Vec<u64> = transfer_from.iter().map(|(_, _, amount)| *amount).collect();
    let input_oars: Result<Vec<OpenAssetRecord>, _> =
      transfer_from.iter()
                   .map(|(_, ref ba, _)| open_asset_record(&ba, &key_pair.get_sk_ref()))
                   .collect();
    let input_oars = input_oars?;
    let input_total: u64 = input_amounts.iter().sum();
    let mut partially_consumed_inputs = Vec::new();
    for (input_amount, oar) in input_amounts.iter().zip(input_oars.iter()) {
      if input_amount > oar.get_amount() {
        return Err(PlatformError::InputsError);
      } else if input_amount < oar.get_amount() {
        let ar = AssetRecord::new(oar.get_amount() - input_amount,
                                  *oar.get_asset_type(),
                                  *oar.get_pub_key())?;
        partially_consumed_inputs.push(ar);
      }
    }
    let output_total = transfer_to.iter().fold(0, |acc, (amount, _)| acc + amount);
    if input_total != output_total {
      return Err(PlatformError::InputsError);
    }
    let asset_type = input_oars[0].get_asset_type();
    let output_ars: Result<Vec<AssetRecord>, _> =
      transfer_to.iter()
                 .map(|(amount, ref addr)| AssetRecord::new(*amount, *asset_type, addr.key))
                 .collect();
    let mut output_ars = output_ars?;
    output_ars.append(&mut partially_consumed_inputs);
    self.add_operation_transfer_asset(input_sids, &input_oars, &output_ars)?;
    Ok(self)
  }
}

#[derive(Default, Serialize, Deserialize)]
pub struct TransactionBuilder {
  txn: Transaction,
  outputs: u64,
}

impl BuildsTransactions for TransactionBuilder {
  fn transaction(&self) -> &Transaction {
    &self.txn
  }
  fn add_operation_create_asset(&mut self,
                                pub_key: &IssuerPublicKey,
                                priv_key: &XfrSecretKey,
                                token_code: Option<AssetTypeCode>,
                                updatable: bool,
                                traceable: bool,
                                _memo: &str)
                                -> Result<&mut Self, PlatformError> {
    self.txn.add_operation(Operation::DefineAsset(DefineAsset::new(DefineAssetBody::new(&token_code.unwrap_or_else(AssetTypeCode::gen_random), pub_key, updatable, traceable, None, Some(ConfidentialMemo {}))?, pub_key, priv_key)?));
    Ok(self)
  }
  fn add_operation_issue_asset(&mut self,
                               pub_key: &IssuerPublicKey,
                               priv_key: &XfrSecretKey,
                               token_code: &AssetTypeCode,
                               seq_num: u64,
                               records: &[TxOutput])
                               -> Result<&mut Self, PlatformError> {
    self.txn
        .add_operation(Operation::IssueAsset(IssueAsset::new(IssueAssetBody::new(token_code,
                                                                                 seq_num,
                                                                                 records)?,
                                                             pub_key,
                                                             priv_key)?));
    Ok(self)
  }
  fn add_operation_transfer_asset(&mut self,
                                  input_sids: Vec<TxoRef>,
                                  input_records: &[OpenAssetRecord],
                                  output_records: &[AssetRecord])
                                  -> Result<&mut Self, PlatformError> {
    // TODO(joe/noah): keep a prng around somewhere?
    let mut prng: ChaChaRng;
    prng = ChaChaRng::from_seed([0u8; 32]);

    self.txn.add_operation(Operation::TransferAsset(TransferAsset::new(TransferAssetBody::new(&mut prng, input_sids, input_records, output_records)?, TransferType::Standard)?));
    Ok(self)
  }

  fn add_operation(&mut self, op: Operation) -> &mut Self {
    self.txn.add_operation(op);
    self
  }

  fn serialize(&self) -> Result<Vec<u8>, PlatformError> {
    let j = serde_json::to_string(&self.txn)?;
    Ok(j.as_bytes().to_vec())
  }

  fn serialize_str(&self) -> Result<String, PlatformError> {
    if let Ok(serialized) = serde_json::to_string(&self.txn) {
      Ok(serialized)
    } else {
      Err(PlatformError::SerializationError)
    }
  }
}

#[derive(Serialize, Deserialize)]
pub struct TransferOperationBuilder {
  input_sids: Vec<TxoRef>,
  input_records: Vec<OpenAssetRecord>,
  spend_amounts: Vec<u64>, //Amount of each input record to spend, the rest will be refunded
  output_records: Vec<AssetRecord>,
  transfer: Option<TransferAsset>,
  transfer_type: TransferType,
}

impl TransferOperationBuilder {
  pub fn new() -> Self {
    TransferOperationBuilder { input_sids: Vec::new(),
                               input_records: Vec::new(),
                               output_records: Vec::new(),
                               spend_amounts: Vec::new(),
                               transfer: None,
                               transfer_type: TransferType::Standard }
  }

  // TxoRef is the location of the input on the ledger and the amount is how much of the record
  // should be spent in the transfer. See tests for example usage.
  pub fn add_input(&mut self,
                   txo_sid: TxoRef,
                   open_ar: OpenAssetRecord,
                   amount: u64)
                   -> Result<&mut Self, PlatformError> {
    if self.transfer.is_some() {
      return Err(PlatformError::InvariantError(Some("Cannot mutate a transfer that has been signed".to_string())));
    }
    self.input_sids.push(txo_sid);
    self.input_records.push(open_ar);
    self.spend_amounts.push(amount);
    Ok(self)
  }

  pub fn add_output(&mut self,
                    amount: u64,
                    recipient: &XfrPublicKey,
                    code: AssetTypeCode)
                    -> Result<&mut Self, PlatformError> {
    if self.transfer.is_some() {
      return Err(PlatformError::InvariantError(Some("Cannot mutate a transfer that has been signed".to_string())));
    }
    self.output_records
        .push(AssetRecord::new(amount, code.val, *recipient).unwrap());
    Ok(self)
  }

  // Ensures that outputs and inputs are balanced by adding remainder outputs for leftover asset
  // amounts
  pub fn balance(&mut self) -> Result<&mut Self, PlatformError> {
    if self.transfer.is_some() {
      return Err(PlatformError::InvariantError(Some("Cannot mutate a transfer that has been signed".to_string())));
    }
    let spend_total: u64 = self.spend_amounts.iter().sum();
    let mut partially_consumed_inputs = Vec::new();
    for (spend_amount, oar) in self.spend_amounts.iter().zip(self.input_records.iter()) {
      if spend_amount > oar.get_amount() {
        return Err(PlatformError::InputsError);
      } else if spend_amount < oar.get_amount() {
        let ar = AssetRecord::new(oar.get_amount() - spend_amount,
                                  *oar.get_asset_type(),
                                  *oar.get_pub_key())?;
        partially_consumed_inputs.push(ar);
      }
    }
    let output_total = self.output_records
                           .iter()
                           .fold(0, |acc, ar| acc + ar.amount);
    if spend_total != output_total {
      return Err(PlatformError::InputsError);
    }
    self.output_records.append(&mut partially_consumed_inputs);
    Ok(self)
  }

  // Finalize the transaction and prepare for signing.
  pub fn create(&mut self, transfer_type: TransferType) -> Result<&mut Self, PlatformError> {
    let mut prng = ChaChaRng::from_seed([0u8; 32]);
    let body = TransferAssetBody::new(&mut prng,
                                      self.input_sids.clone(),
                                      &self.input_records,
                                      &self.output_records)?;
    self.transfer = Some(TransferAsset::new(body, transfer_type)?);
    Ok(self)
  }

  pub fn sign(&mut self, kp: &XfrKeyPair) -> Result<&mut Self, PlatformError> {
    if self.transfer.is_none() {
      return Err(PlatformError::InvariantError(Some("Transaction has not yet been finalized".to_string())));
    }
    let mut new_transfer = self.transfer.as_ref().unwrap().clone();
    new_transfer.sign(&kp);
    self.transfer = Some(new_transfer);
    Ok(self)
  }

  pub fn transaction(&self) -> Result<Operation, PlatformError> {
    if self.transfer.is_none() {
      return Err(PlatformError::InvariantError(Some("Must create transfer".to_string())));
    }
    Ok(Operation::TransferAsset(self.transfer.clone().unwrap()))
  }

  // Checks to see whether all necessary signatures are present and valid
  pub fn validate_signatures(&mut self) -> Result<&mut Self, PlatformError> {
    if self.transfer.is_none() {
      return Err(PlatformError::InvariantError(Some("Transaction has not yet been finalized".to_string())));
    }

    let trn = self.transfer.as_ref().unwrap();
    let mut sig_keys = HashSet::new();
    for sig in &trn.body_signatures {
      if !sig.verify(&serde_json::to_vec(&trn.body).unwrap()) {
        return Err(PlatformError::InvariantError(Some("Invalid signature".to_string())));
      }
      sig_keys.insert(sig.address.key.zei_to_bytes());
    }

    // (1b) all input record owners have signed
    for record in &trn.body.transfer.inputs {
      if !sig_keys.contains(&record.public_key.zei_to_bytes()) {
        return Err(PlatformError::InvariantError(Some("Not all signatures present".to_string())));
      }
    }
    Ok(self)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use ledger::data_model::TxoRef;
  use quickcheck::{Arbitrary, Gen};
  use quickcheck_macros::quickcheck;
  use rand::{Rng, SeedableRng};
  use rand_chacha::ChaChaRng;
  use zei::serialization::ZeiFromToBytes;
  use zei::setup::PublicParams;
  use zei::xfr::asset_record::{build_blind_asset_record, open_asset_record};
  use zei::xfr::lib::{gen_xfr_note, verify_xfr_note};
  use zei::xfr::sig::XfrKeyPair;
  use zei::xfr::structs::{AssetRecord, OpenAssetRecord};

  // Defines an asset type
  #[derive(Clone, Debug, Eq, PartialEq)]
  struct AssetType(pub u8);

  #[derive(Clone, Debug, Eq, PartialEq)]
  struct KeyPair(pub u8);

  #[derive(Clone, Debug, Eq, PartialEq)]
  struct TxoReference(pub u64);

  // Defines an input record
  // (type, amount, conf_type, conf_amount, traceable)
  #[derive(Clone, Debug, Eq, PartialEq)]
  struct InputRecord(pub u64, pub AssetType, pub bool, pub bool, pub bool);

  // Defines an output record
  // (amount, asset type, keypair)
  #[derive(Clone, Debug, Eq, PartialEq)]
  struct OutputRecord(pub u64, pub AssetType, pub KeyPair);

  impl Arbitrary for OutputRecord {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
      OutputRecord(u64::arbitrary(g),
                   AssetType::arbitrary(g),
                   KeyPair::arbitrary(g))
    }
    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
      Box::new(self.0
                   .shrink()
                   .zip(self.1.shrink())
                   .zip(self.2.shrink())
                   .map(|((amount, asset_type), key_pair)| {
                     OutputRecord(amount, asset_type, key_pair)
                   }))
    }
  }

  impl Arbitrary for InputRecord {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
      InputRecord(u64::arbitrary(g),
                  AssetType::arbitrary(g),
                  bool::arbitrary(g),
                  bool::arbitrary(g),
                  bool::arbitrary(g))
    }
    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
      Box::new(self.0
                   .shrink()
                   .zip(self.1.shrink())
                   .zip(self.2.shrink())
                   .zip(self.3.shrink())
                   .zip(self.4.shrink())
                   .map(|((((amount, asset_type), conf_type), conf_amount), traceable)| {
                          InputRecord(amount, asset_type, conf_type, conf_amount, traceable)
                        }))
    }
  }

  impl Arbitrary for AssetType {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
      AssetType(u8::arbitrary(g))
    }
    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
      Box::new(self.0.shrink().map(AssetType))
    }
  }

  impl Arbitrary for TxoReference {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
      TxoReference(g.gen::<u64>() % 10)
    }
    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
      Box::new(self.0.shrink().map(TxoReference))
    }
  }

  impl Arbitrary for KeyPair {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
      // We can generate 10 possible key pairs
      KeyPair(g.gen::<u8>() % 10)
    }

    fn shrink(&self) -> Box<dyn Iterator<Item = Self>> {
      Box::new(self.0.shrink().map(KeyPair))
    }
  }

  #[quickcheck]
  #[ignore]
  fn test_compose_transfer_txn(inputs: Vec<InputRecord>,
                               outputs: Vec<OutputRecord>,
                               key_pair: KeyPair,
                               input_sids: Vec<TxoReference>) {
    let mut prng = ChaChaRng::from_seed([0u8; 32]);
    let params = PublicParams::new();

    //TODO: noah asset records should be buildable by reference
    let key_pair = XfrKeyPair::generate(&mut ChaChaRng::from_seed([key_pair.0; 32]));
    let key_pair_copy = XfrKeyPair::zei_from_bytes(&key_pair.zei_to_bytes());

    // Compose input records
    let input_records: Result<Vec<OpenAssetRecord>, _> =
      inputs.iter()
            .map(|InputRecord(amount, asset_type, conf_type, conf_amount, _)| {
                   let ar = AssetRecord::new(*amount,
                                             [asset_type.0; 16],
                                             *key_pair_copy.get_pk_ref()).unwrap();
                   let ba = build_blind_asset_record(&mut prng,
                                                     &params.pc_gens,
                                                     &ar,
                                                     *conf_type,
                                                     *conf_amount,
                                                     &None);
                   return open_asset_record(&ba, &key_pair.get_sk_ref());
                 })
            .collect();

    // Compose output records
    let output_records: Result<Vec<AssetRecord>, _> =
      outputs.iter()
             .map(|OutputRecord(amount, asset_type, key_pair)| {
               let key_pair = XfrKeyPair::generate(&mut ChaChaRng::from_seed([key_pair.0; 32]));
               AssetRecord::new(*amount, [asset_type.0; 16], *key_pair.get_pk_ref())
             })
             .collect();

    let _input_sids: Vec<TxoRef> = input_sids.iter()
                                             .map(|TxoReference(sid)| TxoRef::Relative(*sid))
                                             .collect();
    let id_proofs = vec![];
    let note = gen_xfr_note(&mut prng,
                            &input_records.unwrap(),
                            &output_records.unwrap(),
                            &[key_pair],
                            &id_proofs);
    if let Ok(xfr_note) = note {
      let null_policies = vec![];
      assert!(verify_xfr_note(&mut prng, &xfr_note, &null_policies).is_ok())
    }
  }

  #[test]
  fn test_transfer_op_builder() -> Result<(), PlatformError> {
    let mut prng = ChaChaRng::from_seed([0u8; 32]);
    let params = PublicParams::new();
    let code_1 = AssetTypeCode::gen_random();
    let code_2 = AssetTypeCode::gen_random();
    let alice = XfrKeyPair::generate(&mut prng);
    let bob = XfrKeyPair::generate(&mut prng);
    let charlie = XfrKeyPair::generate(&mut prng);
    let ben = XfrKeyPair::generate(&mut prng);

    let ar_1 = AssetRecord::new(1000, code_1.val, *alice.get_pk_ref()).unwrap();
    let ar_2 = AssetRecord::new(1000, code_2.val, *bob.get_pk_ref()).unwrap();
    let ba_1 = build_blind_asset_record(&mut prng, &params.pc_gens, &ar_1, false, false, &None);
    let ba_2 = build_blind_asset_record(&mut prng, &params.pc_gens, &ar_2, false, false, &None);

    // Attempt to spend too much
    let mut invalid_outputs_transfer_op = TransferOperationBuilder::new();
    let res =
      invalid_outputs_transfer_op.add_input(TxoRef::Relative(1),
                                            open_asset_record(&ba_1, alice.get_sk_ref()).unwrap(),
                                            20)?
                                 .add_output(25, bob.get_pk_ref(), code_1)?
                                 .balance();

    assert!(res.is_err());

    // Change transaction after signing
    let mut invalid_sig_op = TransferOperationBuilder::new();
    let res = invalid_sig_op.add_input(TxoRef::Relative(1),
                                       open_asset_record(&ba_1, alice.get_sk_ref()).unwrap(),
                                       20)?
                            .add_output(20, bob.get_pk_ref(), code_1)?
                            .balance()?
                            .create(TransferType::Standard)?
                            .sign(&alice)?
                            .add_output(20, bob.get_pk_ref(), code_1);
    assert!(res.is_err());

    // Not all signatures present
    let mut missing_sig_op = TransferOperationBuilder::new();
    let res = missing_sig_op.add_input(TxoRef::Relative(1),
                                       open_asset_record(&ba_1, alice.get_sk_ref()).unwrap(),
                                       20)?
                            .add_output(20, bob.get_pk_ref(), code_1)?
                            .balance()?
                            .create(TransferType::Standard)?
                            .validate_signatures();

    assert!(&res.is_err());

    // Finally, test a valid transfer
    let _valid_transfer_op =
      TransferOperationBuilder::new()
      .add_input(TxoRef::Relative(1), open_asset_record(&ba_1, alice.get_sk_ref()).unwrap(), 20)?
      .add_input(TxoRef::Relative(2), open_asset_record(&ba_2, bob.get_sk_ref()).unwrap(), 20)?
      .add_output(5, bob.get_pk_ref(), code_1)?
      .add_output(13, charlie.get_pk_ref(), code_1)?
      .add_output(2, ben.get_pk_ref(), code_1)?
      .add_output(5, bob.get_pk_ref(), code_2)?
      .add_output(13, charlie.get_pk_ref(), code_2)?
      .add_output(2, ben.get_pk_ref(), code_2)?
      .balance()?
      .create(TransferType::Standard)?
      .sign(&alice)?
      .sign(&bob)?
      .transaction()?;

    Ok(())
  }
}
