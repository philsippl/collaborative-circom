use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use bytes::Bytes;
use mpc_core::protocols::rep3::id::PartyID;
use mpc_core::protocols::rep3::network::Rep3Network;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

pub struct Rep3TestNetwork {
    p1_p2_sender: UnboundedSender<Bytes>,
    p1_p3_sender: UnboundedSender<Bytes>,
    p2_p3_sender: UnboundedSender<Bytes>,
    p2_p1_sender: UnboundedSender<Bytes>,
    p3_p1_sender: UnboundedSender<Bytes>,
    p3_p2_sender: UnboundedSender<Bytes>,
    p1_p2_receiver: UnboundedReceiver<Bytes>,
    p1_p3_receiver: UnboundedReceiver<Bytes>,
    p2_p3_receiver: UnboundedReceiver<Bytes>,
    p2_p1_receiver: UnboundedReceiver<Bytes>,
    p3_p1_receiver: UnboundedReceiver<Bytes>,
    p3_p2_receiver: UnboundedReceiver<Bytes>,
}

impl Default for Rep3TestNetwork {
    fn default() -> Self {
        Self::new()
    }
}

impl Rep3TestNetwork {
    pub fn new() -> Self {
        // AT Most 1 message is buffered before they are read so this should be fine
        let p1_p2 = mpsc::unbounded_channel();
        let p1_p3 = mpsc::unbounded_channel();
        let p2_p3 = mpsc::unbounded_channel();
        let p2_p1 = mpsc::unbounded_channel();
        let p3_p1 = mpsc::unbounded_channel();
        let p3_p2 = mpsc::unbounded_channel();

        Self {
            p1_p2_sender: p1_p2.0,
            p1_p3_sender: p1_p3.0,
            p2_p1_sender: p2_p1.0,
            p2_p3_sender: p2_p3.0,
            p3_p1_sender: p3_p1.0,
            p3_p2_sender: p3_p2.0,
            p1_p2_receiver: p1_p2.1,
            p1_p3_receiver: p1_p3.1,
            p2_p1_receiver: p2_p1.1,
            p2_p3_receiver: p2_p3.1,
            p3_p1_receiver: p3_p1.1,
            p3_p2_receiver: p3_p2.1,
        }
    }

    pub fn get_party_networks(self) -> [PartyTestNetwork; 3] {
        let party1 = PartyTestNetwork {
            id: PartyID::ID0,
            send_prev: self.p1_p3_sender,
            recv_prev: self.p3_p1_receiver,
            send_next: self.p1_p2_sender,
            recv_next: self.p2_p1_receiver,
            _stats: [0; 4],
        };

        let party2 = PartyTestNetwork {
            id: PartyID::ID1,
            send_prev: self.p2_p1_sender,
            recv_prev: self.p1_p2_receiver,
            send_next: self.p2_p3_sender,
            recv_next: self.p3_p2_receiver,
            _stats: [0; 4],
        };

        let party3 = PartyTestNetwork {
            id: PartyID::ID2,
            send_prev: self.p3_p2_sender,
            recv_prev: self.p2_p3_receiver,
            send_next: self.p3_p1_sender,
            recv_next: self.p1_p3_receiver,
            _stats: [0; 4],
        };

        [party1, party2, party3]
    }
}

#[derive(Debug)]
pub struct PartyTestNetwork {
    pub(crate) id: PartyID,
    pub(crate) send_prev: UnboundedSender<Bytes>,
    pub(crate) send_next: UnboundedSender<Bytes>,
    pub(crate) recv_prev: UnboundedReceiver<Bytes>,
    pub(crate) recv_next: UnboundedReceiver<Bytes>,
    pub(crate) _stats: [usize; 4], // [sent_prev, sent_next, recv_prev, recv_next]
}

impl Rep3Network for PartyTestNetwork {
    fn get_id(&self) -> PartyID {
        self.id
    }

    fn send_many<F: CanonicalSerialize>(
        &mut self,
        target: PartyID,
        data: &[F],
    ) -> std::io::Result<()> {
        let size = data.serialized_size(ark_serialize::Compress::No);
        let mut to_send = Vec::with_capacity(size);
        data.serialize_uncompressed(&mut to_send).unwrap();
        if self.id.next_id() == target {
            self.send_next
                .send(Bytes::from(to_send))
                .expect("can send to next")
        } else if self.id.prev_id() == target {
            self.send_prev
                .send(Bytes::from(to_send))
                .expect("can send to next");
        } else {
            panic!("You want to send to yourself?")
        }
        Ok(())
    }

    fn recv_many<F: CanonicalDeserialize>(&mut self, from: PartyID) -> std::io::Result<Vec<F>> {
        if self.id.next_id() == from {
            let data = Vec::from(self.recv_next.blocking_recv().unwrap());
            Ok(Vec::<F>::deserialize_uncompressed(data.as_slice()).unwrap())
        } else if self.id.prev_id() == from {
            let data = Vec::from(self.recv_prev.blocking_recv().unwrap());
            Ok(Vec::<F>::deserialize_uncompressed(data.as_slice()).unwrap())
        } else {
            panic!("You want to read from yourself?")
        }
    }
}
mod field_share {
    use crate::protocols::rep3::Rep3TestNetwork;
    use ark_ff::Field;
    use ark_std::{UniformRand, Zero};
    use itertools::izip;
    use mpc_core::protocols::rep3::witness_extension_impl::Rep3VmType;
    use mpc_core::protocols::rep3::Rep3PrimeFieldShare;
    use mpc_core::protocols::rep3::{self, fieldshare::Rep3PrimeFieldShareVec, Rep3Protocol};
    use mpc_core::traits::CircomWitnessExtensionProtocol;
    use mpc_core::traits::PrimeFieldMpcProtocol;
    use rand::thread_rng;
    use std::{collections::HashSet, thread};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn rep3_add() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::rand(&mut rng);
        let y = ark_bn254::Fr::rand(&mut rng);
        let x_shares = rep3::utils::share_field_element(x, &mut rng);
        let y_shares = rep3::utils::share_field_element(y, &mut rng);
        let should_result = x + y;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter().zip(y_shares))
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.add(&x, &y))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_sub() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::rand(&mut rng);
        let y = ark_bn254::Fr::rand(&mut rng);
        let x_shares = rep3::utils::share_field_element(x, &mut rng);
        let y_shares = rep3::utils::share_field_element(y, &mut rng);
        let should_result = x - y;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter().zip(y_shares))
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.sub(&x, &y))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }
    #[tokio::test]
    async fn rep3_mul2_then_add() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::rand(&mut rng);
        let y = ark_bn254::Fr::rand(&mut rng);
        let x_shares = rep3::utils::share_field_element(x, &mut rng);
        let y_shares = rep3::utils::share_field_element(y, &mut rng);
        let should_result = ((x * y) * y) + x;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter().zip(y_shares))
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                let mul = rep3.mul(&x, &y).unwrap();
                let mul = rep3.mul(&mul, &y).unwrap();
                tx.send(rep3.add(&mul, &x))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    use std::str::FromStr;
    #[tokio::test]
    async fn rep3_mul_vec_bn() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = [
            ark_bn254::Fr::from_str(
                "13839525561076761625780930844889299788193703994911163378019280196128582690055",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "19302971480864839163158232064620707211435225928426123775531639309944891593977",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "8048717310762513532550620831072439583505607813129662608591015555880153427210",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "2585271390974436123003027749932103593962191064365118925254473311197989280023",
            )
            .unwrap(),
        ];
        let y = [
            ark_bn254::Fr::from_str(
                "2688648969035332064113669477511029957484512453056743431884706385750388613065",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "13632770404954969699480437686769008635735921498648460325387842712839596176806",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "19199593902803943133889170931116903997086625101975591190159463567024116566625",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "8255472466884305547009533395117607586789669747151273739964395707537515634749",
            )
            .unwrap(),
        ];
        let should_result = vec![
            ark_bn254::Fr::from_str(
                "14012338922664984944451142760937475581748095944353358534203030914664561190462",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "4297594441150501195973997511775989720904927516253689527653694984160382713321",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "7875903949174289914141782934879682497141865775307179984684659764891697566272",
            )
            .unwrap(),
            ark_bn254::Fr::from_str(
                "6646526994769136778802685410292764833027657364709823469005920616147071273574",
            )
            .unwrap(),
        ];
        let mut x_shares1 = vec![];
        let mut x_shares2 = vec![];
        let mut x_shares3 = vec![];
        let mut y_shares1 = vec![];
        let mut y_shares2 = vec![];
        let mut y_shares3 = vec![];
        for (x, y) in x.iter().zip(y.iter()) {
            let [x1, x2, x3] = rep3::utils::share_field_element(*x, &mut rng);
            let [y1, y2, y3] = rep3::utils::share_field_element(*y, &mut rng);
            x_shares1.push(x1);
            x_shares2.push(x2);
            x_shares3.push(x3);
            y_shares1.push(y1);
            y_shares2.push(y2);
            y_shares3.push(y3);
        }

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(
                [
                    Rep3PrimeFieldShareVec::from(x_shares1),
                    Rep3PrimeFieldShareVec::from(x_shares2),
                    Rep3PrimeFieldShareVec::from(x_shares3),
                ]
                .into_iter()
                .zip([
                    Rep3PrimeFieldShareVec::from(y_shares1),
                    Rep3PrimeFieldShareVec::from(y_shares2),
                    Rep3PrimeFieldShareVec::from(y_shares3),
                ]),
            )
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();

                let mul = rep3.mul_vec(&x, &y).unwrap();
                tx.send(mul)
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_elements(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_mul_vec() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = (0..1)
            .map(|_| ark_bn254::Fr::from_str("2").unwrap())
            .collect::<Vec<_>>();
        let y = (0..1)
            .map(|_| ark_bn254::Fr::from_str("3").unwrap())
            .collect::<Vec<_>>();
        let mut x_shares1 = vec![];
        let mut x_shares2 = vec![];
        let mut x_shares3 = vec![];
        let mut y_shares1 = vec![];
        let mut y_shares2 = vec![];
        let mut y_shares3 = vec![];
        let mut should_result = vec![];
        for (x, y) in x.iter().zip(y.iter()) {
            let [x1, x2, x3] = rep3::utils::share_field_element(*x, &mut rng);
            let [y1, y2, y3] = rep3::utils::share_field_element(*y, &mut rng);
            x_shares1.push(x1);
            x_shares2.push(x2);
            x_shares3.push(x3);
            y_shares1.push(y1);
            y_shares2.push(y2);
            y_shares3.push(y3);
            should_result.push((x * y) * y);
        }

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(
                [
                    Rep3PrimeFieldShareVec::from(x_shares1),
                    Rep3PrimeFieldShareVec::from(x_shares2),
                    Rep3PrimeFieldShareVec::from(x_shares3),
                ]
                .into_iter()
                .zip([
                    Rep3PrimeFieldShareVec::from(y_shares1),
                    Rep3PrimeFieldShareVec::from(y_shares2),
                    Rep3PrimeFieldShareVec::from(y_shares3),
                ]),
            )
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();

                let mul = rep3.mul_vec(&x, &y).unwrap();
                let mul = rep3.mul_vec(&mul, &y).unwrap();
                tx.send(mul)
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_elements(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_neg() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::rand(&mut rng);
        let x_shares = rep3::utils::share_field_element(x, &mut rng);
        let should_result = -x;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), x) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.neg(&x))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_inv() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let mut x = ark_bn254::Fr::rand(&mut rng);
        while x.is_zero() {
            x = ark_bn254::Fr::rand(&mut rng);
        }
        let x_shares = rep3::utils::share_field_element(x, &mut rng);
        let should_result = x.inverse().unwrap();
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), x) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.inv(&x).unwrap())
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_sqrt() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x_ = ark_bn254::Fr::rand(&mut rng);
        let x = x_.square(); // Guarantees a square root exists
        let x_shares = rep3::utils::share_field_element(x, &mut rng);
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), x) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.sqrt(&x).unwrap())
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert!(is_result == x_ || is_result == -x_);
    }

    macro_rules! bool_op_test {
        ($name: ident, $op: tt) => {
            #[tokio::test]
            async fn $name() {
                let constant_number = ark_bn254::Fr::from_str("50").unwrap();
                for i in -1..=1 {
                    let compare = constant_number + ark_bn254::Fr::from(i);
                    let test_network = Rep3TestNetwork::default();
                    let mut rng = thread_rng();
                    let x_shares = rep3::utils::share_field_element(constant_number, &mut rng);
                    let y_shares = rep3::utils::share_field_element(compare, &mut rng);
                    let should_result = ark_bn254::Fr::from(constant_number $op compare);
                    let (tx1, rx1) = oneshot::channel();
                    let (tx2, rx2) = oneshot::channel();
                    let (tx3, rx3) = oneshot::channel();
                    for (net, tx, x_share, y_share, x_pub, y_pub) in izip!(
                        test_network.get_party_networks(),
                        [tx1, tx2, tx3],
                        x_shares,
                        y_shares,
                        vec![Rep3VmType::Public(constant_number); 3],
                        vec![Rep3VmType::Public(compare); 3]
                    ) {
                        thread::spawn(move || {
                            let mut rep3 = Rep3Protocol::new(net).unwrap();
                            let x = Rep3VmType::Shared(x_share);
                            let y = Rep3VmType::Shared(y_share);

                            let shared_compare = rep3.$name(x.clone(), y.clone()).unwrap();
                            let rhs_const = rep3.$name(x, y_pub.clone()).unwrap();
                            let lhs_const = rep3.$name(x_pub.clone(), y).unwrap();
                            let both_const = rep3.$name(x_pub, y_pub).unwrap();
                            tx.send([both_const, shared_compare, rhs_const, lhs_const])
                        });
                    }
                    let results1 = rx1.await.unwrap();
                    let results2 = rx2.await.unwrap();
                    let results3 = rx3.await.unwrap();
                    for (result1, result2, result3) in izip!(results1, results2, results3) {
                        match (result1, result2, result3) {
                            (
                                Rep3VmType::Shared(a),
                                Rep3VmType::Shared(b),
                                Rep3VmType::Shared(c),
                            ) => {
                                let is_result = rep3::utils::combine_field_element(a, b, c);
                                println!("{constant_number} {} {compare} = {is_result}", stringify!($op));
                                assert_eq!(is_result, should_result);
                            }
                            (
                                Rep3VmType::Public(a),
                                Rep3VmType::Public(b),
                                Rep3VmType::Public(c),
                            ) => {
                                assert_eq!(a, b);
                                assert_eq!(b, c);
                                println!("{constant_number} {} {compare} = {a}", stringify!($op));
                                assert_eq!(a, should_result);
                            }
                            _ => panic!("must be shared"),
                        }
                    }
                }
            }
        };
    }
    bool_op_test!(vm_lt, <);
    bool_op_test!(vm_le, <=);
    bool_op_test!(vm_gt, >);
    bool_op_test!(vm_ge, >=);

    #[tokio::test]
    async fn rep3_a2b_zero() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::zero();
        let x_shares = rep3::utils::share_field_element(x, &mut rng);

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), x) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.a2b(&x).unwrap())
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::xor_combine_biguint(result1, result2, result3);

        let should_result = x.into();
        assert_eq!(is_result, should_result);
        let is_result_f: ark_bn254::Fr = is_result.into();
        assert_eq!(is_result_f, x);
    }
    #[tokio::test]
    async fn rep3_a2b() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::rand(&mut rng);
        let x_shares = rep3::utils::share_field_element(x, &mut rng);

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), x) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.a2b(&x).unwrap())
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::xor_combine_biguint(result1, result2, result3);

        let should_result = x.into();
        assert_eq!(is_result, should_result);
        let is_result_f: ark_bn254::Fr = is_result.into();
        assert_eq!(is_result_f, x);
    }

    #[tokio::test]
    async fn rep3_b2a() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::Fr::rand(&mut rng);
        let x_shares = rep3::utils::xor_share_biguint(x, &mut rng);

        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), x) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.b2a(x).unwrap())
            });
        }
        let result1: Rep3PrimeFieldShare<ark_bn254::Fr> = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_field_element(result1, result2, result3);
        assert_eq!(is_result, x);
    }

    #[tokio::test]
    async fn rep3_random() {
        let test_network = Rep3TestNetwork::default();
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for (net, tx) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::<ark_bn254::Fr, _>::new(net).unwrap();
                tx.send((0..10).map(|_| rep3.rand().unwrap()).collect::<Vec<_>>())
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        assert_eq!(result1.iter().collect::<HashSet<_>>().len(), 10);
        assert_eq!(result2.iter().collect::<HashSet<_>>().len(), 10);
        assert_eq!(result3.iter().collect::<HashSet<_>>().len(), 10);
        for ((s1, s2), s3) in result1.into_iter().zip(result2).zip(result3) {
            let (s1a, s1b) = s1.ab();
            let (s2a, s2b) = s2.ab();
            let (s3a, s3b) = s3.ab();
            assert_eq!(s1a, s2b);
            assert_eq!(s2a, s3b);
            assert_eq!(s3a, s1b);
        }
    }
}

mod curve_share {
    use ark_std::UniformRand;
    use std::thread;

    use mpc_core::protocols::rep3::{self, Rep3Protocol};
    use rand::thread_rng;
    use tokio::sync::oneshot;

    use crate::protocols::rep3::Rep3TestNetwork;
    use mpc_core::traits::EcMpcProtocol;

    #[tokio::test]
    async fn rep3_add() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::G1Projective::rand(&mut rng);
        let y = ark_bn254::G1Projective::rand(&mut rng);
        let x_shares = rep3::utils::share_curve_point(x, &mut rng);
        let y_shares = rep3::utils::share_curve_point(y, &mut rng);
        let should_result = x + y;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter().zip(y_shares))
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.add_points(&x, &y))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_curve_point(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_sub() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let x = ark_bn254::G1Projective::rand(&mut rng);
        let y = ark_bn254::G1Projective::rand(&mut rng);
        let x_shares = rep3::utils::share_curve_point(x, &mut rng);
        let y_shares = rep3::utils::share_curve_point(y, &mut rng);
        let should_result = x - y;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), (x, y)) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(x_shares.into_iter().zip(y_shares))
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.sub_points(&x, &y))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_curve_point(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_scalar_mul_public_point() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let public_point = ark_bn254::G1Projective::rand(&mut rng);
        let scalar = ark_bn254::Fr::rand(&mut rng);
        let scalar_shares = rep3::utils::share_field_element(scalar, &mut rng);
        let should_result = public_point * scalar;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), scalar) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(scalar_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.scalar_mul_public_point(&public_point, &scalar))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_curve_point(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }

    #[tokio::test]
    async fn rep3_scalar_mul_public_scalar() {
        let test_network = Rep3TestNetwork::default();
        let mut rng = thread_rng();
        let point = ark_bn254::G1Projective::rand(&mut rng);
        let public_scalar = ark_bn254::Fr::rand(&mut rng);
        let point_shares = rep3::utils::share_curve_point(point, &mut rng);
        let should_result = point * public_scalar;
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        let (tx3, rx3) = oneshot::channel();
        for ((net, tx), point) in test_network
            .get_party_networks()
            .into_iter()
            .zip([tx1, tx2, tx3])
            .zip(point_shares.into_iter())
        {
            thread::spawn(move || {
                let mut rep3 = Rep3Protocol::new(net).unwrap();
                tx.send(rep3.scalar_mul_public_scalar(&point, &public_scalar))
            });
        }
        let result1 = rx1.await.unwrap();
        let result2 = rx2.await.unwrap();
        let result3 = rx3.await.unwrap();
        let is_result = rep3::utils::combine_curve_point(result1, result2, result3);
        assert_eq!(is_result, should_result);
    }
}
