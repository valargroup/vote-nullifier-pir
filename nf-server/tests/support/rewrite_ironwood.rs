//! Forge transaction effects while preserving structural decodability for rejection tests.
use std::sync::Arc;
use zakura_chain::transaction::Transaction;

pub fn rewrite(tx: &mut Arc<Transaction>, omit_action: bool) -> bool {
    if tx.ironwood_actions().count() < if omit_action { 2 } else { 1 } {
        return false;
    }
    #[cfg(not(feature = "upstream"))]
    {
        if let Transaction::V6 {
            ironwood_shielded_data: Some(bundle),
            ..
        } = Arc::make_mut(tx)
        {
            if omit_action {
                bundle.actions = bundle
                    .actions
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                bundle.proof.0.truncate(bundle.proof.0.len() - 2272);
            } else {
                bundle.actions.iter_mut().next().unwrap().action.nullifier =
                    [0; 32].try_into().unwrap();
            }
        } else {
            panic!("fixture must contain a V6 Ironwood transaction");
        }
    }
    #[cfg(feature = "upstream")]
    {
        use upstream_orchard as orchard;
        use upstream_primitives::transaction;
        use zakura_chain::serialization::{ZcashDeserialize, ZcashSerialize};
        let mut bytes = Vec::new();
        tx.zcash_serialize(&mut bytes).unwrap();
        let data = transaction::Transaction::read(bytes.as_slice(), tx.consensus_branch_id())
            .unwrap()
            .into_data();
        let forged = data
            .map_bundles::<transaction::Authorized>(
                |b| b,
                |b| b,
                |bundle| {
                    bundle.map(|bundle| {
                        if bundle.bundle_version().value_pool() != orchard::ValuePool::Ironwood {
                            return bundle;
                        }
                        let mut actions: Vec<_> = bundle.actions().iter().cloned().collect();
                        let mut proof = bundle.authorization().proof().as_ref().to_vec();
                        if omit_action {
                            actions.remove(0);
                            proof.truncate(proof.len() - 2272);
                        } else {
                            let action = &actions[0];
                            actions[0] = orchard::Action::from_parts(
                                orchard::note::Nullifier::from_bytes(&[0; 32]).unwrap(),
                                action.rk().clone(),
                                *action.cmx(),
                                action.encrypted_note().clone(),
                                action.cv_net().clone(),
                                action.authorization().clone(),
                            )
                            .unwrap();
                        }
                        orchard::Bundle::try_from_parts(
                            nonempty::NonEmpty::from_vec(actions).unwrap(),
                            *bundle.flags(),
                            *bundle.value_balance(),
                            *bundle.anchor(),
                            orchard::bundle::Authorized::from_parts(
                                orchard::Proof::new(proof),
                                bundle.authorization().binding_signature().clone(),
                            ),
                            bundle.bundle_version(),
                        )
                        .unwrap()
                    })
                },
            )
            .freeze()
            .unwrap();
        bytes.clear();
        forged.write(&mut bytes).unwrap();
        *tx = Arc::new(Transaction::zcash_deserialize(bytes.as_slice()).unwrap());
    }
    true
}
