pub mod format;
pub mod raw;

use anyhow::anyhow;
use format::*;
use futures::prelude::*;
use raw::RawRepository;
use serde::{Deserialize, Serialize};
use simperby_common::reserved::ReservedState;
use simperby_common::verify::CommitSequenceVerifier;
use simperby_common::*;
use simperby_network::{NetworkConfig, Peer, SharedKnownPeers};
use std::fmt;

pub type Branch = String;
pub type Tag = String;

pub const FINALIZED_BRANCH_NAME: &str = "finalized";
pub const WORK_BRANCH_NAME: &str = "work";
pub const FP_BRANCH_NAME: &str = "fp";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Clone, Serialize, Deserialize, Hash)]
pub struct CommitHash {
    pub hash: [u8; 20],
}

impl fmt::Display for CommitHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?")
    }
}

pub type Error = anyhow::Error;

/// The local Simperby blockchain data repository.
///
/// It automatically locks the repository once created.
///
/// - It **verifies** all the incoming changes and applies them to the local repository
/// only if they are valid.
pub struct DistributedRepository<T> {
    raw: T,
}

fn get_timestamp() -> Timestamp {
    let now = std::time::SystemTime::now();
    let since_the_epoch = now.duration_since(std::time::UNIX_EPOCH).unwrap();
    since_the_epoch.as_millis() as Timestamp
}

impl<T: RawRepository> DistributedRepository<T> {
    pub async fn new(raw: T) -> Result<Self, Error> {
        Ok(Self { raw })
    }

    /// Initializes the genesis repository from the genesis commit,
    /// leaving a genesis header.
    ///
    /// The repository MUST have only two commits: `initial` and `genesis` in the `finalized` branch.
    /// The `genesis` commit MUST have set the initial reserved state in a valid format.
    ///
    /// It also
    /// - creates `fp` branch and its commit (for the genesis block).
    /// - creates `work` branch at the same place with the `finalized` branch.
    pub async fn genesis(&mut self) -> Result<(), Error> {
        unimplemented!()
    }

    /// Returns the block header from the `finalized` branch.
    pub async fn get_last_finalized_block_header(&self) -> Result<BlockHeader, Error> {
        let commit_hash = self.raw.locate_branch(FINALIZED_BRANCH_NAME.into()).await?;
        let semantic_commit = self.raw.read_semantic_commit(commit_hash).await?;
        let commit = format::from_semantic_commit(semantic_commit).map_err(|e| anyhow!(e))?;
        if let Commit::Block(block_header) = commit {
            Ok(block_header)
        } else {
            Err(anyhow!(
                "repository integrity broken; `finalized` branch is not on a block"
            ))
        }
    }

    /// Returns the reserved state from the `finalized` branch.
    pub async fn get_reserved_state(&self) -> Result<ReservedState, Error> {
        self.raw.read_reserved_state().await.map_err(|e| anyhow!(e))
    }

    /// Cleans all the outdated commits, remote repositories and branches.
    ///
    /// It will leave only
    /// - the `finalized` branch
    /// - the `work` branch
    /// - the `fp` branch.
    ///
    /// and
    /// - the `p` branch
    /// - the `a-#` branches
    /// - the `b-#` branches
    /// if only the branches are not outdated (branched from the last finalized commit).
    pub async fn clean(&mut self) -> Result<(), Error> {
        let finalized_branch_commit_hash =
            self.raw.locate_branch(FINALIZED_BRANCH_NAME.into()).await?;

        let branches = self.raw.list_branches().await?;

        // delete outdated p branch, a-# branches, b-# branches
        for branch in branches {
            if !(branch.as_str() == WORK_BRANCH_NAME
                || branch.as_str() == FINALIZED_BRANCH_NAME
                || branch.as_str() == FP_BRANCH_NAME)
            {
                let branch_commit = self.raw.locate_branch(branch.clone()).await?;

                if finalized_branch_commit_hash
                    != self
                        .raw
                        .find_merge_base(branch_commit, finalized_branch_commit_hash)
                        .await?
                {
                    self.raw.delete_branch(branch.to_string()).await?;
                }
            }
        }

        // remove remote branches
        let remote_list = self.raw.list_remotes().await?;
        for (remote_name, _) in remote_list {
            self.raw.remove_remote(remote_name).await?;
        }

        // TODO : CSV

        Ok(())
    }

    /// Fetches new commits from the network.
    ///
    /// It **verifies** all the incoming changes and applies them to the local repository
    /// only if they are valid.
    ///
    /// - It may move the `finalized` branch.
    /// - It may add some `a-#` branches.
    /// - It may add some `b-#` branches.
    ///
    /// It may leave some remote repository (representing each peer) after the operation.
    pub async fn fetch(
        &mut self,
        _network_config: &NetworkConfig,
        known_peers: &[Peer],
    ) -> Result<(), Error> {
        for _peer in known_peers {
            self.raw
                .add_remote("yoonho".into(), "github".into())
                .await?;

            // TODO: change yoonho, github into something else with peer
            // It will be evaluated by "git" method
        }

        self.raw.fetch_all().await?;

        let branches_after_fetch = self.raw.list_branches().await?;

        // Make block_header_vec to contain BlockHeaders below,
        // and make block_commit_hash_vec to contain CommitHash
        let mut block_header_vec: Vec<BlockHeader> = vec![];
        let mut finalized_branches: Vec<Branch> = vec![];

        // For all incoming branches
        for branch in branches_after_fetch {
            // Delete branches which merge base is not the known finalized commit
            let branch_commit_hash = self.raw.locate_branch(branch.clone()).await?;
            let finalized_branch_commit_hash =
                self.raw.locate_branch(FINALIZED_BRANCH_NAME.into()).await?;

            if finalized_branch_commit_hash
                != self
                    .raw
                    .find_merge_base(branch_commit_hash, finalized_branch_commit_hash)
                    .await?
            {
                self.raw.delete_branch(branch.to_string()).await?;
                // Jump to the next branch
                continue;
            }

            // Make new CSV for the branch
            let blockheader = self.get_last_finalized_block_header().await?;
            let branch_reserved_state = self.get_reserved_state().await?;
            let mut branch_csv =
                verify::CommitSequenceVerifier::new(blockheader.clone(), branch_reserved_state)
                    .unwrap();

            // Find new commits (after the finalized commit)
            let branch_semantic_commit = self.raw.read_semantic_commit(branch_commit_hash).await?;
            let _branch_commit =
                format::from_semantic_commit(branch_semantic_commit).map_err(|e| anyhow!(e))?;

            let branch_ancestors = self.raw.list_ancestors(branch_commit_hash, None).await?;
            let finalized_ancestors = self
                .raw
                .list_ancestors(finalized_branch_commit_hash, None)
                .await?;

            let new_commits_hashes: Vec<CommitHash> = branch_ancestors
                .into_iter()
                .filter(|hash| !finalized_ancestors.contains(hash))
                .collect();

            let new_commits_hashes_cloned = new_commits_hashes.clone();
            let tip_commit_hash = new_commits_hashes_cloned.last().unwrap();
            let tip_semantic_commit = self.raw.read_semantic_commit(*tip_commit_hash).await?;
            let tip_commit =
                format::from_semantic_commit(tip_semantic_commit).map_err(|e| anyhow!(e))?;

            for new_commit_hash in new_commits_hashes {
                // Verify with CSV about every new commits
                let semantic_commit = self.raw.read_semantic_commit(new_commit_hash).await?;
                let new_commit =
                    format::from_semantic_commit(semantic_commit).map_err(|e| anyhow!(e))?;
                verify::CommitSequenceVerifier::apply_commit(&mut branch_csv, &new_commit).unwrap();
            }

            // If new commit is Agenda commit or AgendaProof commit
            if let Commit::Agenda(_) = tip_commit {
                let branch_name = "a-#"; //TODO : change # into number
                self.raw
                    .create_branch(branch_name.to_string(), *tip_commit_hash)
                    .await?;
            } else if let Commit::AgendaProof(_) = tip_commit {
                let branch_name = "a-#"; //TODO : change # into number
                self.raw
                    .create_branch(branch_name.to_string(), *tip_commit_hash)
                    .await?;
            } else if let Commit::Block(commit_block_header) = tip_commit {
                // Else if new commit is Block commit, add block_header, block_header_hash into the vector
                // Then we find fp, if there is fp then move the finalized branch
                let fp_commit_hash = self.raw.locate_branch(FP_BRANCH_NAME.into()).await?;
                let fp_semantic_commit = self.raw.read_semantic_commit(fp_commit_hash).await?;
                let finalization_proof: FinalizationProof =
                    serde_json::from_str(&fp_semantic_commit.body).unwrap();
                let result = verify::verify_finalization_proof(
                    block_header_vec.get(0).unwrap(),
                    &finalization_proof,
                );
                // If the finalization proof is right, we push it to a vector to check
                // If we can't find the right finalization proof, we create a new b# branch
                match result {
                    Ok(()) => {
                        block_header_vec.push(commit_block_header);
                        finalized_branches.push(branch);
                    }
                    Err(_) => {
                        self.raw
                            .create_branch("b-#".to_string(), *tip_commit_hash)
                            .await?
                    }
                };
            }
            // For fast-forward branches
            else {
                self.raw.move_branch(branch, *tip_commit_hash).await?;
            }
        }
        // Check the finalized branch's tip block commit height
        // Delete all branches which is finalized but do not have the highest height
        // Panic if there is 2 or more same height finalized branches
        // Else we move the finalized branch
        let block_heights: Vec<u64> = block_header_vec
            .iter()
            .map(|blockheader| blockheader.height)
            .collect();
        let highest_block_height = *block_heights.iter().max().unwrap();
        let mut same_height_finalized_block_count = 0;
        let mut survived_finalized_branch_index = None;
        for (index, block_height) in block_heights.iter().enumerate() {
            if highest_block_height == *block_height {
                same_height_finalized_block_count += 1;
                survived_finalized_branch_index = Some(index);
            } else {
                let to_be_deleted_branch_name = finalized_branches.get(index).unwrap().clone();
                self.raw.delete_branch(to_be_deleted_branch_name).await?;
            }
        }
        if same_height_finalized_block_count > 1 {
            panic!("chain forked");
        } else {
            let survived_branch_name = finalized_branches
                .get(survived_finalized_branch_index.unwrap())
                .unwrap()
                .clone();
            let survived_tip_commit = self.raw.locate_branch(survived_branch_name).await?;
            self.raw
                .move_branch(FINALIZED_BRANCH_NAME.to_string(), survived_tip_commit)
                .await?;
        }
        //TODO: update fp branch

        Ok(())
    }

    /// Serves the distributed repository protocol indefinitely.
    /// It **verifies** all the incoming changes and applies them to the local repository
    /// only if they are valid.
    pub async fn serve(
        self,
        _network_config: &NetworkConfig,
        _peers: SharedKnownPeers,
    ) -> Result<tokio::task::JoinHandle<Result<(), Error>>, Error> {
        unimplemented!()
    }

    /// Checks the validity of the repository, starting from the given height.
    ///
    /// It checks
    /// 1. all the reserved branches and tags
    /// 2. the finalization proof in the `fp` branch.
    /// 3. the existence of merge commits
    /// 4. the canonical history of the `finalized` branch.
    /// 5. the reserved state in a valid format.
    pub async fn check(&self, _starting_height: BlockHeight) -> Result<bool, Error> {
        unimplemented!()
    }

    /// Synchronizes the `finalized` branch to the given commit.
    ///
    /// This will verify every commit along the way.
    /// If the given commit is not a descendant of the
    /// current `finalized` (i.e., cannot be fast-forwarded), it fails.
    ///
    /// Note that the proof in the `fp` branch must be set for the candidate commit
    /// for the last finalized block, which is `block_commit`.
    pub async fn sync(&mut self, _block_commit: &CommitHash) -> Result<(), Error> {
        unimplemented!()
    }
    /// Returns the currently valid and height-acceptable agendas in the repository.
    pub async fn get_agendas(&self) -> Result<Vec<(CommitHash, Hash256)>, Error> {
        unimplemented!()
    }

    /// Returns the currently valid and height-acceptable blocks in the repository.
    pub async fn get_blocks(&self) -> Result<Vec<(CommitHash, Hash256)>, Error> {
        unimplemented!()
    }

    /// Finalizes a single block and moves the `finalized` branch to it, and updates the `fp` branch.
    ///
    /// It will verify the finalization proof and the commits.
    /// The difference between `finalize` and `sync` is that `sync` doesn't update the `fp` branch,
    /// but checks it.
    pub async fn finalize(
        &mut self,
        _block_commit_hash: &CommitHash,
        _proof: &FinalizationProof,
    ) -> Result<(), Error> {
        unimplemented!()
    }

    /// Informs that the given agenda has been approved.
    pub async fn approve(
        &mut self,
        _agenda_commit_hash: &CommitHash,
        _proof: Vec<(PublicKey, TypedSignature<Agenda>)>,
    ) -> Result<CommitHash, Error> {
        unimplemented!()
    }

    /// Creates an agenda commit on top of the `work` branch.
    pub async fn create_agenda(&mut self, author: PublicKey) -> Result<CommitHash, Error> {
        let last_header = self.get_last_finalized_block_header().await?;
        let work_commit = self.raw.locate_branch(WORK_BRANCH_NAME.into()).await?;
        let last_header_commit = self.raw.locate_branch(FINALIZED_BRANCH_NAME.into()).await?;

        // Check if the `work` branch is rebased on top of the `finalized` branch.
        if self
            .raw
            .find_merge_base(last_header_commit, work_commit)
            .await?
            != last_header_commit
        {
            return Err(anyhow!(
                "branch {} should be rebased on {}",
                WORK_BRANCH_NAME,
                FINALIZED_BRANCH_NAME
            ));
        }

        // Fetch and convert commits
        let commits = self.raw.list_ancestors(work_commit, Some(256)).await?;
        let position = commits
            .iter()
            .position(|c| *c == last_header_commit)
            .expect("TODO: handle the case where it exceeds the limit.");

        // commits starting from the very next one to the last finalized block.
        let commits = stream::iter(commits.iter().take(position).rev().cloned().map(|c| {
            let raw = &self.raw;
            async move { raw.read_semantic_commit(c).await.map(|x| (x, c)) }
        }))
        .buffered(256)
        .collect::<Vec<_>>()
        .await;
        let commits = commits.into_iter().collect::<Result<Vec<_>, _>>()?;
        let commits = commits
            .into_iter()
            .map(|(commit, hash)| {
                from_semantic_commit(commit)
                    .map_err(|e| (e, hash))
                    .map(|x| (x, hash))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|(error, hash)| anyhow!("failed to convert the commit {}: {}", hash, error))?;

        // Check the validity of the commit sequence
        let reserved_state = self.get_reserved_state().await?;
        let mut verifier = CommitSequenceVerifier::new(last_header.clone(), reserved_state)
            .map_err(|e| anyhow!("verification error on commit {}: {}", last_header_commit, e))?;
        for (commit, hash) in commits.iter() {
            verifier
                .apply_commit(commit)
                .map_err(|e| anyhow!("verification error on commit {}: {}", hash, e))?;
        }

        // Check whether the commit sequence is in the transaction phase.
        let mut transactions = Vec::new();

        for (commit, _) in commits {
            if let Commit::Transaction(t) = commit {
                transactions.push(t.clone());
            } else {
                return Err(anyhow!(
                    "branch {} is not in the transaction phase",
                    WORK_BRANCH_NAME
                ));
            }
        }

        let agenda_commit = Commit::Agenda(Agenda {
            author,
            timestamp: get_timestamp(),
            hash: Agenda::calculate_hash(last_header.height + 1, &transactions),
            height: last_header.height + 1,
        });
        let semantic_commit = to_semantic_commit(&agenda_commit, &last_header);

        self.raw.checkout_clean().await?;
        self.raw.checkout(WORK_BRANCH_NAME.into()).await?;
        let result = self.raw.create_semantic_commit(semantic_commit).await?;
        Ok(result)
    }

    /// Puts a 'vote' tag on the commit.
    pub async fn vote(&mut self, _commit: CommitHash) -> Result<(), Error> {
        unimplemented!()
    }

    /// Puts a 'veto' tag on the commit.
    pub async fn veto(&mut self, _commit: CommitHash) -> Result<(), Error> {
        unimplemented!()
    }

    /// Creates a block commit on top of the `work` branch.
    pub async fn create_block(&mut self, _author: PublicKey) -> Result<CommitHash, Error> {
        unimplemented!()
    }

    /// Creates an agenda commit on top of the `work` branch.
    pub async fn create_extra_agenda_transaction(
        &mut self,
        _transaction: &ExtraAgendaTransaction,
    ) -> Result<CommitHash, Error> {
        unimplemented!()
    }
}
