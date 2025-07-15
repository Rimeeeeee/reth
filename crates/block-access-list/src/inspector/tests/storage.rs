//! Tests for `StorageChangeInspector`

use crate::inspector::storage::StorageChangeInspector;

use alloy_primitives::{hex, Address, U256};
use revm::{
    context::TxEnv,
    context_interface::{
        result::{ExecutionResult, Output},
        ContextTr, TransactTo,
    },
    database::CacheDB,
    database_interface::EmptyDB,
    handler::EvmTr,
    primitives::hardfork::SpecId,
    state::AccountInfo,
    Context, DatabaseCommit, InspectEvm, MainBuilder, MainContext,
};

#[test]
fn test_counter_storage_change_inspector() {
    /*
    contract Counter {
        uint256 public storedData;

        function setNumber(uint256 x) public {
            storedData = x;
        }

        function increment() public {
            storedData = storedData + 1;
        }
    }
    */
    //! Result
    //! Deployed contract: 0xbd770416a3345f91e4b34576cb804a576fa48eb1
    /*
    OPCODE: 55
    Not Found /// Not Found means no pre value is found
    set_result: Success { reason: Stop, gas_used: 43718, gas_refunded: 0, logs: [], output: Call(0x) }
    == setNumber(42) Writes ==
    {
        0xbd770416a3345f91e4b34576cb804a576fa48eb1: {
            0x000000000000000000000000000000000000000000000000000000000000002a: (
                0,
                0,
            ),
        },
    }
    == setNumber(42) Reads ==
    {}
    OPCODE: 54
    OPCODE: 55
    Not Found
    inc_result: Success { reason: Stop, gas_used: 43526, gas_refunded: 0, logs: [], output: Call(0x) }
    == increment() Writes ==
    {
        0xbd770416a3345f91e4b34576cb804a576fa48eb1: {
            0x0000000000000000000000000000000000000000000000000000000000000001: (
                0,
                0,
            ),
        },
    }
    == increment() Reads ==
    {
        0xbd770416a3345f91e4b34576cb804a576fa48eb1: {
            0x0000000000000000000000000000000000000000000000000000000000000000,
        },
    }
        */

    let deployer = Address::ZERO;
    let code = hex!("0x6080604052348015600e575f5ffd5b506101ca8061001c5f395ff3fe608060405234801561000f575f5ffd5b506004361061003f575f3560e01c80632a1afcd9146100435780633fb5c1cb14610061578063d09de08a1461007d575b5f5ffd5b61004b610087565b60405161005891906100c2565b60405180910390f35b61007b60048036038101906100769190610109565b61008c565b005b610085610095565b005b5f5481565b805f8190555050565b60015f546100a39190610161565b5f81905550565b5f819050919050565b6100bc816100aa565b82525050565b5f6020820190506100d55f8301846100b3565b92915050565b5f5ffd5b6100e8816100aa565b81146100f2575f5ffd5b50565b5f81359050610103816100df565b92915050565b5f6020828403121561011e5761011d6100db565b5b5f61012b848285016100f5565b91505092915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f61016b826100aa565b9150610176836100aa565b925082820190508082111561018e5761018d610134565b5b9291505056fea2646970667358221220c281b50f79c1e72e5ac83d10b85f65f491d2bf1f41954612cef2ddb77713b35864736f6c634300081c0033");
    let mut db = CacheDB::new(EmptyDB::default());
    db.insert_account_info(
        deployer,
        AccountInfo { balance: U256::from(1_000_000_000u64), ..Default::default() },
    );
    let context =
        Context::mainnet().with_db(db).modify_cfg_chained(|cfg| cfg.spec = SpecId::CANCUN);

    let mut insp_deploy = StorageChangeInspector::new();
    let _contract = {
        let mut evm = context.clone().build_mainnet_with_inspector(&mut insp_deploy);
        let deploy_result = evm
            .inspect_tx(TxEnv {
                caller: deployer,
                gas_limit: 1_000_000,
                kind: TransactTo::Create,
                data: code.into(),
                ..Default::default()
            })
            .unwrap();

        let contract = match deploy_result.result {
            ExecutionResult::Success { output: Output::Create(_, Some(addr)), .. } => addr,
            _ => panic!("Contract deployment failed"),
        };

        evm.ctx().db_mut().commit(deploy_result.state);
        let db_after_deploy = evm.ctx().db().clone();
        let context = context.with_db(db_after_deploy);

        println!("Deployed contract: {:?}", contract);

        let mut insp_set = StorageChangeInspector::new();
        {
            let mut evm = context.clone().build_mainnet_with_inspector(&mut insp_set);
            let set_number =
                hex!("3fb5c1cb000000000000000000000000000000000000000000000000000000000000002a"); // setNumber(42)
            let set_result = evm
                .inspect_tx(TxEnv {
                    caller: deployer,
                    gas_limit: 500_000,
                    kind: TransactTo::Call(contract),
                    data: set_number.into(),
                    nonce: 1,
                    ..Default::default()
                })
                .unwrap();
            println!("set_result: {:?}", set_result.result);
            evm.ctx().db_mut().commit(set_result.state);
        }

        println!("== setNumber(42) Writes ==");
        println!("{:#?}", insp_set.writes());
        println!("== setNumber(42) Reads ==");
        println!("{:#?}", insp_set.reads());

        let mut insp_inc = StorageChangeInspector::new();
        {
            let mut evm = context.build_mainnet_with_inspector(&mut insp_inc);
            let increment = hex!("d09de08a"); // increment()
            let inc_result = evm
                .inspect_tx(TxEnv {
                    caller: deployer,
                    gas_limit: 500_000,
                    kind: TransactTo::Call(contract),
                    data: increment.into(),
                    nonce: 1,
                    ..Default::default()
                })
                .unwrap();
            println!("inc_result: {:?}", inc_result.result);
            evm.ctx().db_mut().commit(inc_result.state);
        }

        println!("== increment() Writes ==");
        println!("{:#?}", insp_inc.writes());
        println!("== increment() Reads ==");
        println!("{:#?}", insp_inc.reads());

        contract
    };
}
