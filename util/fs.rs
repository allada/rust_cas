// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::collections::HashMap;
use std::fs::Metadata;
use std::io::IoSlice;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;

use error::{make_err, Code, Error, ResultExt};
use log;
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf, SeekFrom};
use tokio::sync::{Mutex, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock, Semaphore, SemaphorePermit};

/// A lock obtained for a FileSlot depending on whether read or read-write
/// access was requested for the slot.
#[derive(Debug)]
pub enum FileLock {
    ReadOnly(OwnedRwLockReadGuard<()>),
    ReadWrite(OwnedRwLockWriteGuard<()>),
}

/// We wrap all tokio::fs items in our own wrapper so we can limit the number of outstanding
/// open files at any given time. This will greatly reduce the chance we'll hit open file limit
/// issues.

#[derive(Debug)]
pub struct FileSlot<'a> {
    // We hold the permit because once it is dropped it goes back into the queue.
    _permit: SemaphorePermit<'a>,
    inner: tokio::fs::File,
}

impl<'a> AsRef<tokio::fs::File> for FileSlot<'a> {
    fn as_ref(&self) -> &tokio::fs::File {
        &self.inner
    }
}

impl<'a> AsMut<tokio::fs::File> for FileSlot<'a> {
    fn as_mut(&mut self) -> &mut tokio::fs::File {
        &mut self.inner
    }
}

impl<'a> AsyncRead for FileSlot<'a> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<'a> AsyncSeek for FileSlot<'a> {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> Result<(), tokio::io::Error> {
        Pin::new(&mut self.inner).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u64, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_complete(cx)
    }
}

impl<'a> AsyncWrite for FileSlot<'a> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

const DEFAULT_OPEN_FILE_PERMITS: usize = 10;
static OPEN_FILE_SEMAPHORE: Semaphore = Semaphore::const_new(DEFAULT_OPEN_FILE_PERMITS);
static OPEN_FILE_LOCKS: Mutex<Option<HashMap<String, Arc<RwLock<()>>>>> = Mutex::const_new(None);

async fn get_lock(path: impl AsRef<Path>) -> Arc<RwLock<()>> {
    let mut locks = OPEN_FILE_LOCKS.lock().await;
    let map = locks.get_or_insert_with(|| HashMap::with_capacity(OPEN_FILE_SEMAPHORE.available_permits() + 6));
    let lock = map
        .entry(path.as_ref().to_string_lossy().to_string())
        .or_insert_with(|| Arc::new(RwLock::new(())))
        .clone();
    // While we hold the mutex, perform garbage collection on it, the clone
    // above ensures that we have a count of at least 2 on the current RWLock.
    // We only perform this garbage collection when the map is reaching capacity
    // to try and limit how often we have to iterate over the map as it is a
    // slow operation.
    if map.capacity() - map.len() < 5 {
        map.retain(|_, v| Arc::strong_count(&v) > 1);
    }
    lock
}

async fn take_lock(path: impl AsRef<Path>, write: bool) -> FileLock {
    // Purposely separated the get_lock function to ensure Mutex is dropped once
    // we have an Arc to the RwLock.
    let lock = get_lock(path).await;
    if write {
        FileLock::ReadWrite(lock.write_owned().await)
    } else {
        FileLock::ReadOnly(lock.read_owned().await)
    }
}

async fn take_open_semaphore() -> Result<SemaphorePermit<'static>, Error> {
    OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))
}

pub fn set_open_file_limit(limit: usize) {
    if limit < DEFAULT_OPEN_FILE_PERMITS {
        log::error!(
            "set_open_file_limit({}) must be greater than {}",
            limit,
            DEFAULT_OPEN_FILE_PERMITS
        );
        return;
    }
    OPEN_FILE_SEMAPHORE.add_permits(limit - DEFAULT_OPEN_FILE_PERMITS);
}

pub async fn open_file_read_locked(path: impl AsRef<Path> + std::fmt::Debug) -> Result<FileSlot<'static>, Error> {
    let permit = take_open_semaphore().await?;
    let _file_permit = take_lock(&path, false).await;
    Ok(FileSlot {
        _permit: permit,
        inner: tokio::fs::File::open(&path)
            .await
            .err_tip(|| format!("Could not open {:?}", path))?,
    })
}

pub async fn open_file(path: impl AsRef<Path> + std::fmt::Debug) -> Result<FileSlot<'static>, Error> {
    let permit = take_open_semaphore().await?;
    Ok(FileSlot {
        _permit: permit,
        inner: tokio::fs::File::open(&path)
            .await
            .err_tip(|| format!("Could not open {:?}", path))?,
    })
}

pub async fn create_file(path: impl AsRef<Path> + std::fmt::Debug) -> Result<FileSlot<'static>, Error> {
    let permit = take_open_semaphore().await?;
    Ok(FileSlot {
        _permit: permit,
        inner: tokio::fs::File::create(&path)
            .await
            .err_tip(|| format!("Could not open {:?}", path))?,
    })
}

pub async fn hard_link_src_locked(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    let _src_file_permit = take_lock(&src, false).await;
    tokio::fs::hard_link(src, dst).await.map_err(|e| e.into())
}

pub async fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::hard_link(src, dst).await.map_err(|e| e.into())
}

pub async fn set_permissions(src: impl AsRef<Path>, perm: std::fs::Permissions) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::set_permissions(src, perm).await.map_err(|e| e.into())
}

pub async fn create_dir(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::create_dir(path).await.map_err(|e| e.into())
}

pub async fn create_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::create_dir_all(path).await.map_err(|e| e.into())
}

pub async fn symlink(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::symlink(src, dst).await.map_err(|e| e.into())
}

pub async fn read_link(path: impl AsRef<Path>) -> Result<std::path::PathBuf, Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::read_link(path).await.map_err(|e| e.into())
}

pub struct ReadDir<'a> {
    // We hold the permit because once it is dropped it goes back into the queue.
    permit: SemaphorePermit<'a>,
    inner: tokio::fs::ReadDir,
}

impl<'a> ReadDir<'a> {
    pub fn into_inner(self) -> (SemaphorePermit<'a>, tokio::fs::ReadDir) {
        (self.permit, self.inner)
    }
}

impl<'a> AsRef<tokio::fs::ReadDir> for ReadDir<'a> {
    fn as_ref(&self) -> &tokio::fs::ReadDir {
        &self.inner
    }
}

impl<'a> AsMut<tokio::fs::ReadDir> for ReadDir<'a> {
    fn as_mut(&mut self) -> &mut tokio::fs::ReadDir {
        &mut self.inner
    }
}

pub async fn read_dir(path: impl AsRef<Path>) -> Result<ReadDir<'static>, Error> {
    let permit = take_open_semaphore().await?;
    Ok(ReadDir {
        permit,
        inner: tokio::fs::read_dir(path).await.map_err(|e| Into::<Error>::into(e))?,
    })
}

pub async fn rename_dest_locked(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    let _to_file_permit = take_lock(&to, true).await;
    tokio::fs::rename(from, to).await.map_err(|e| e.into())
}

pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::rename(from, to).await.map_err(|e| e.into())
}

pub async fn remove_file_locked(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    let _file_permit = take_lock(&path, true).await;
    tokio::fs::remove_file(path).await.map_err(|e| e.into())
}

pub async fn remove_file(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::remove_file(path).await.map_err(|e| e.into())
}

pub async fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::canonicalize(path).await.map_err(|e| e.into())
}

pub async fn metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::metadata(path).await.map_err(|e| e.into())
}

pub async fn read(path: impl AsRef<Path> + std::fmt::Debug) -> Result<Vec<u8>, Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::read(path).await.map_err(|e| e.into())
}

pub async fn symlink_metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::symlink_metadata(path).await.map_err(|e| e.into())
}

pub async fn remove_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = take_open_semaphore().await?;
    tokio::fs::remove_dir_all(path).await.map_err(|e| e.into())
}
