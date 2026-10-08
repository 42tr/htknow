// 与后端 ArchiveFormat 保持一致。
export function archiveFormat(filename) {
  const suffix = filename?.toLowerCase().match(/\.(zip|rar|7z|tar\.gz|tar\.bz2|tar\.xz|tar|tgz)$/)?.[1]
  return suffix === 'tgz' ? 'TAR.GZ' : suffix?.toUpperCase() || ''
}
