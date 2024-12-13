// Copyright 2022 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// This file is @generated by prost-build.
/// Provides control over what strategies (local, remote, etc) may be used.
///
/// An empty policies (e.g. unset) implies no enforcement, anything is allowed.
///
/// Policies are enforced against both user-provided values (flags) and
/// application-internal defaults. The latter is useful for guarding against
/// unexpectedly hard-coded defaults.
///
/// Sample usage to allow everything to execute remotely, while only allowing
/// genrules to execute locally:
///
///    strategy_policy {
///      mnemonic_policy {
///        default_allowlist: \["remote"\]
///        strategy_allowlist: [
///          { mnemonic: "Genrule" strategy: \["local"\] }
///        ]
///      }
///    }
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StrategyPolicy {
    /// Controls per-mnemonic policies for regular spawn/action execution. Relevant
    /// command-line flags this controls include --strategy and --genrule_strategy.
    #[prost(message, optional, tag = "1")]
    pub mnemonic_policy: ::core::option::Option<MnemonicPolicy>,
    /// Controls per-mnemonic policies for the remote execution leg of dynamic
    /// execution. Relevant flag is --dynamic_remote_strategy.
    #[prost(message, optional, tag = "2")]
    pub dynamic_remote_policy: ::core::option::Option<MnemonicPolicy>,
    /// Controls per-mnemonic policies for the local execution leg of dynamic
    /// execution. Relevant flag is --dynamic_local_strategy.
    #[prost(message, optional, tag = "3")]
    pub dynamic_local_policy: ::core::option::Option<MnemonicPolicy>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MnemonicPolicy {
    /// Default allowed strategies for mnemonics not present in `strategy` list.
    #[prost(string, repeated, tag = "1")]
    pub default_allowlist: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
    #[prost(message, repeated, tag = "2")]
    pub strategy_allowlist: ::prost::alloc::vec::Vec<StrategiesForMnemonic>,
}
/// Per-mnemonic allowlist settings.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct StrategiesForMnemonic {
    #[prost(string, optional, tag = "1")]
    pub mnemonic: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(string, repeated, tag = "2")]
    pub strategy: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
}