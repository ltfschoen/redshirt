// Copyright(c) 2019 Pierre Krieger

use crate::signature::Signature;
use core::{convert::TryFrom, fmt, str::FromStr};
use sha2::{digest::FixedOutput as _, Digest as _};

/// Definition of an interface.
// TODO: remove?
pub struct Interface {
    name: String,
    functions: Vec<Function>,
    hash: InterfaceHash,
}

/// Prototype of an interface being built.
pub struct InterfaceBuilder {
    name: String,
    functions: Vec<Function>,
}

/// Identifier of an interface. Can be either a hash or a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InterfaceId {
    Hash(InterfaceHash),

    /// TODO: docs
    ///
    /// > **Note**: The original design doesn't allow specifying an interface by a name. However
    /// >           this mechanism has been added in order to enable support for programs compiled
    /// >           for WASI for example.
    Bytes(String),
}

/// Hash of an interface definition.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct InterfaceHash([u8; 32]);

struct Function {
    name: String,
    signature: Signature,
}

impl Interface {
    /// Starts building an [`Interface`] with an [`InterfaceBuilder`].
    pub fn new() -> InterfaceBuilder {
        InterfaceBuilder {
            name: String::new(),
            functions: Vec::new(),
        }
    }

    /// Returns the hash of the interface.
    pub fn hash(&self) -> &InterfaceHash {
        &self.hash
    }
}

impl InterfaceBuilder {
    /// Changes the name of the prototype interface.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Adds a function to the prototype interface.
    // TODO: don't expose wasmi types in the API
    pub fn with_function(
        mut self,
        name: impl Into<String>,
        signature: impl Into<Signature>,
    ) -> Self {
        self.functions.push(Function {
            name: name.into(),
            signature: signature.into(),
        });
        self
    }

    /// Turns the builder into an actual interface.
    pub fn build(mut self) -> Interface {
        self.functions.shrink_to_fit();

        // Let's build the hash of our interface.
        let mut hash_state = sha2::Sha256::default();
        hash_state.input(self.name.as_bytes());
        // TODO: hash the function definitions as well
        // TODO: need some delimiter between elements of the hash, otherwise people can craft
        //       collisions

        Interface {
            name: self.name,
            functions: self.functions,
            hash: InterfaceHash(hash_state.fixed_result().into()),
        }
    }
}

impl From<InterfaceHash> for InterfaceId {
    fn from(hash: InterfaceHash) -> InterfaceId {
        InterfaceId::Hash(hash)
    }
}

impl From<[u8; 32]> for InterfaceId {
    fn from(hash: [u8; 32]) -> InterfaceId {
        InterfaceId::Hash(hash.into())
    }
}

impl From<String> for InterfaceId {
    fn from(name: String) -> InterfaceId {
        InterfaceId::Bytes(name)
    }
}

impl<'a> From<&'a str> for InterfaceId {
    fn from(name: &'a str) -> InterfaceId {
        InterfaceId::Bytes(name.to_owned())
    }
}

impl From<[u8; 32]> for InterfaceHash {
    fn from(hash: [u8; 32]) -> InterfaceHash {
        InterfaceHash(hash)
    }
}

impl<'a> TryFrom<&'a [u8]> for InterfaceHash {
    type Error = ();
    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() != 32 {
            return Err(());
        }
        let mut hash = [0; 32];
        hash.copy_from_slice(value);
        Ok(InterfaceHash(hash))
    }
}

impl FromStr for InterfaceHash {
    type Err = bs58::decode::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut tmp_out = [0; 32];
        let len = bs58::decode(s).into(&mut tmp_out)?;
        debug_assert!(len <= 32);
        let mut real_out = [0; 32];
        real_out[32 - len..].copy_from_slice(&tmp_out[..len]);
        Ok(InterfaceHash(real_out))
    }
}

impl fmt::Display for InterfaceHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&bs58::encode(&self.0).into_string(), f)
    }
}

impl fmt::Debug for InterfaceHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "InterfaceHash({})", bs58::encode(&self.0).into_string())
    }
}

// TODO: test that displaying and parsing InterfaceHash yields back same result
