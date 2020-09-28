#[macro_use]
extern crate log;

use libp2p::identity::Keypair;
use libp2p::{PeerId, Multiaddr, Swarm, Transport, NetworkBehaviour};
use libp2p::swarm::NetworkBehaviourEventProcess;
use libp2p::mplex::MplexConfig;
use libp2p::plaintext::PlainText2Config;
use libp2p::kad::{Kademlia, KademliaConfig, KademliaEvent, QueryResult, Quorum, GetProvidersError};
use libp2p::kad::record::{Record, Key};
use libp2p::kad::record::store::{MemoryStore, MemoryStoreConfig};
use libp2p::identify::{Identify, IdentifyEvent};
use libp2p::core::upgrade::Version;
use libp2p::mplex::MaxBufferBehaviour;
use std::{io, env};
use std::str::FromStr;
use async_std::task::{Context, Poll, self};
use futures::future;
use futures_util::{StreamExt, TryStreamExt, AsyncBufReadExt};
use rand::{thread_rng, distributions::Alphanumeric, Rng};

#[derive(NetworkBehaviour)]
pub struct MyBehaviour {
  kademlia: Kademlia<MemoryStore>,
  identify: Identify,
  #[behaviour(ignore)]
  peer_id: PeerId,
  #[behaviour(ignore)]
  to_provide: Vec<String>,
  #[behaviour(ignore)]
  trigger_items: Vec<String>,
  #[behaviour(ignore)]
  populate: bool,
  #[behaviour(ignore)]
  trigger_timeout: bool,
  #[behaviour(ignore)]
  poll_again: bool,
}

impl NetworkBehaviourEventProcess<KademliaEvent> for MyBehaviour {
  fn inject_event(&mut self, event: KademliaEvent) {
    match event {
      _ => trace!("Received Kademlia event: {:?}", event)
    };

    match event {
      KademliaEvent::QueryResult { result, .. } => {
        match result {
          QueryResult::GetProviders(result) => {
            debug!("Got kademlia result: {:?}", result);
            self.trigger_timeout = true;
            self.poll_again = true;
            match result {
              Ok(_) => {},
              Err(GetProvidersError::Timeout { key, .. }) => panic!("Timeout happend for query {:?}", key),
            }
          }
          QueryResult::PutRecord(Ok(_)) => {
          },
          QueryResult::StartProviding(Ok(_)) => {
            self.poll_again = true;
            self.populate = true;
          },
          QueryResult::PutRecord(Err(err)) => {
            panic!("Failed to put record: {:?}", err);
          },
          QueryResult::Bootstrap(Ok(_)) => {

          }
          _ => {}
        };
      },
      _ => {}
    }
  }
}

impl NetworkBehaviourEventProcess<IdentifyEvent> for MyBehaviour {
  fn inject_event(&mut self, event: IdentifyEvent) {
    trace!("Received identify event: {:?}", event);

    match event {
      IdentifyEvent::Received { peer_id, info, .. } => {
        for addr in info.listen_addrs {
          self.kademlia.add_address(&peer_id, addr);
        }
      },
      _ => {}
    };
  }
}

fn load_keypair() -> io::Result<Keypair> {
  match env::var("PRIVATE_KEY_PATH") {
    Ok(path) => {
      let mut bytes = std::fs::read(path)?;
      match Keypair::rsa_from_pkcs8(&mut bytes) {
        Ok(keypair) => Ok(keypair),
        Err(decoding_error) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Private key content couldn't be parsed: {:?}", decoding_error))),
      }
    },
    Err(env::VarError::NotPresent) => {
      debug!("PRIVATE_KEY_PATH not defined, generating random key..");
      Ok(Keypair::generate_ed25519())
    },
    Err(env::VarError::NotUnicode(string)) => {
      Err(io::Error::new(io::ErrorKind::InvalidData, format!("Value {:?} is not valid unicode", string)))
    }
  }
}

fn get_bootstrap_servers() -> io::Result<Vec<(PeerId, Multiaddr)>> {
  match env::var("BOOTSTRAP_SERVERS") {
    Ok(val) => {
      let servers = val.split(",").map(|server| {
        let mut splitted = server.split(":");
        let peer_id = PeerId::from_str(splitted.next().unwrap()).unwrap();
        let multiaddr = Multiaddr::from_str(splitted.next().unwrap()).unwrap();

        (peer_id, multiaddr)
      }).collect();

      Ok(servers)
    },
    Err(env::VarError::NotPresent) => Ok(Vec::new()),
    Err(e) => Err(io::Error::new(io::ErrorKind::Other, e))
  }
}

fn main() -> io::Result<()> {
  pretty_env_logger::init_timed();
  let keypair = load_keypair()?;
  let peer_id = PeerId::from(keypair.public());

  let bootstrap_servers = get_bootstrap_servers()?;

  let tcp = libp2p::tcp::TcpConfig::new();
  let plaintext = PlainText2Config{ local_public_key: keypair.public() };
  let mut mplex = MplexConfig::default();
  mplex.max_buffer_len_behaviour(MaxBufferBehaviour::Block);
  mplex.split_send_size(1024 * 1024);
  mplex.max_substreams(100000);
  let mut yamux = libp2p::yamux::Config::default();
  yamux.set_max_num_streams(100000);

  let transport = tcp.upgrade(Version::V1).authenticate(plaintext).multiplex(mplex);

  let mut swarm = {
    // Create a Kademlia behaviour.
    let mut store_config = MemoryStoreConfig::default();
    store_config.max_records = 1024 * 1024;
    store_config.max_provided_keys = 1024 * 1024;
    store_config.max_providers_per_key = 1024; // TODO: check again, there are some replication factor to take into account
    let store = MemoryStore::with_config(peer_id.clone(), store_config);
    let kademlia_config = KademliaConfig::default();
    let kademlia = Kademlia::with_config(peer_id.clone(), store, kademlia_config);
    let identify = Identify::new("/identify/1.0.0".to_string(), "/clevercloud-identify/1.0.0".to_string(), keypair.public());
    let behaviour = MyBehaviour {
      kademlia,
      identify,
      peer_id: peer_id.clone(),
      to_provide: Vec::new(),
      trigger_items: Vec::new(),
      populate: false,
      trigger_timeout: false,
      poll_again: false
    };
    Swarm::new(transport, behaviour, peer_id.clone())
  };

  let bind_to_port = std::env::var("PORT").unwrap_or("0".to_string());
  Swarm::listen_on(&mut swarm, format!("/ip4/0.0.0.0/tcp/{}", bind_to_port).parse().unwrap()).unwrap();

  for server in &bootstrap_servers {
    swarm.kademlia.add_address(&server.0, server.1.clone());
  }

  if bootstrap_servers.len() > 0 {
    swarm.kademlia.bootstrap().expect("Should bootstrap");
  }

  let mut stdin = async_std::io::BufReader::new(async_std::io::stdin()).lines();

  // Kick it off.
  let mut listening = false;
  task::block_on(future::poll_fn(move |cx: &mut Context| {
    trace!("future block on");
    loop {
      match stdin.try_poll_next_unpin(cx) {
          Poll::Ready(Some(line)) => parse_line(line.unwrap(), &mut swarm),
          Poll::Ready(None) => panic!("Stdin closed"),
          Poll::Pending => break
      }
    }
    trace!("after stdin loop");
    loop {
      if swarm.populate {
        put_provider(&mut swarm);
      } else if swarm.trigger_timeout {
        trigger_timeout(&mut swarm);
      }

      let polled = swarm.poll_next_unpin(cx);
      info!("polled swarm(): {:?}", polled);
      match polled {
        Poll::Ready(Some(event)) => debug!("Poll::ready event {:?}", event),
        Poll::Ready(None) => {
          trace!("Got Poll::Ready(None)");
          return Poll::Ready(())
        },
        Poll::Pending => {
          if !listening {
            info!("PeerID: {:?}", swarm.peer_id);
            if let Some(a) = Swarm::listeners(&swarm).next() {
              info!("Listening on {:?}", a);

              let to_join = format!("JOIN {:?} {:?}", swarm.peer_id, a);
              info!("{}", to_join.replace("PeerId(\"", "").replace("\")", "").replace("\"", ""));
              listening = true;
            } else {
              return Poll::Pending;
            }
          }

          if swarm.poll_again {
            swarm.poll_again = false;
            continue;
          }

          return Poll::Pending;
        }
      }
    }
  }));

  Ok(())
}

fn put_provider(swarm: &mut Swarm<MyBehaviour>) {
  swarm.populate = false;
  if let Some(item) = swarm.to_provide.pop() {
    debug!("PUT provider {}. remaining={}", item, swarm.to_provide.len());
    swarm.kademlia.start_providing(Key::from(item.into_bytes())).expect("Should provide");
  }
}

fn trigger_timeout(swarm: &mut Swarm<MyBehaviour>) {
  swarm.trigger_timeout = false;

  if let Some(item) = swarm.trigger_items.pop() {
    debug!("Getting providers for {}", item);
    swarm.kademlia.get_providers(Key::from(item.into_bytes()));
  } else {
    error!("Failed to trigger the bug");
  }
}

fn parse_line(line: String, swarm: &mut Swarm<MyBehaviour>) {
  let mut splitted = line.split(' ');

  match splitted.next() {
    Some("POPULATE") => {
      for _ in 0..1500 {
        let rand_string: String = thread_rng()
          .sample_iter(&Alphanumeric)
          .take(30)
          .collect();

        swarm.to_provide.push(rand_string);
      }

      swarm.populate = true;
    },
    Some("GET") => {
      let key: &str = splitted.next().unwrap();
      swarm.kademlia.get_record(&Key::from(key.as_bytes().to_vec()), Quorum::One);
    },
    Some("GET_PROVIDERS") => {
      let key: &str = splitted.next().unwrap();
      swarm.kademlia.get_providers(Key::from(key.as_bytes().to_vec()));
    }
    Some("PUT") => {
      let key: &str = splitted.next().unwrap();
      let value: &str = splitted.next().unwrap();

      let record = Record::new(key.as_bytes().to_vec(), value.as_bytes().to_vec());
      swarm.kademlia.put_record(record, Quorum::One).expect("Should put record");
    },
    Some("PROVIDE") => {
      let key: &str = splitted.next().unwrap();
      swarm.kademlia.start_providing(Key::from(key.as_bytes().to_vec())).expect("Should start providing");
    },
    _ => error!("Unknown command")
  }
}
