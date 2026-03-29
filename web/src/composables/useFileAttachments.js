import { ref } from 'vue'
import { saveUpload } from '../api/tauri'

const MAX_FILE_SIZE = 50 * 1024 * 1024 // 50MB
const MAX_FILES = 10

export function useFileAttachments() {
  const attachedFiles = ref([])

  function readFileAsDataURI(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader()
      reader.onload = () => resolve(reader.result)
      reader.onerror = reject
      reader.readAsDataURL(file)
    })
  }

  function readFileAsArrayBuffer(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader()
      reader.onload = () => resolve(new Uint8Array(reader.result))
      reader.onerror = reject
      reader.readAsArrayBuffer(file)
    })
  }

  async function addFiles(files) {
    for (const file of files) {
      if (attachedFiles.value.length >= MAX_FILES) break
      if (file.size > MAX_FILE_SIZE) continue
      const entry = { name: file.name, type: file.type, size: file.size, file }
      if (file.type.startsWith('image/')) {
        entry.preview = await readFileAsDataURI(file)
      }
      attachedFiles.value.push(entry)
    }
  }

  function removeFile(index) {
    attachedFiles.value.splice(index, 1)
  }

  function clearFiles() {
    attachedFiles.value = []
  }

  function onFileSelect(e) {
    if (e.target.files) addFiles(Array.from(e.target.files))
    e.target.value = ''
  }

  function onPaste(e) {
    const items = e.clipboardData?.items
    if (!items) return
    const pasteFiles = []
    for (const item of items) {
      if (item.kind === 'file') {
        const file = item.getAsFile()
        if (file) pasteFiles.push(file)
      }
    }
    if (pasteFiles.length) {
      e.preventDefault()
      addFiles(pasteFiles)
    }
  }

  function onDrop(e) {
    e.preventDefault()
    if (e.dataTransfer?.files) addFiles(Array.from(e.dataTransfer.files))
  }

  function onDragOver(e) {
    e.preventDefault()
  }

  function formatFileSize(bytes) {
    if (bytes < 1024) return bytes + 'B'
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + 'KB'
    return (bytes / (1024 * 1024)).toFixed(1) + 'MB'
  }

  async function saveFilesToDisk(entries) {
    const results = []
    for (const entry of entries) {
      try {
        const bytes = await readFileAsArrayBuffer(entry.file)
        const savedPath = await saveUpload(entry.name, bytes)
        results.push({ name: entry.name, path: savedPath, isImage: entry.type.startsWith('image/') })
      } catch (e) {
        console.warn('Failed to save upload:', entry.name, e)
      }
    }
    return results
  }

  function buildContentWithFiles(text, savedFiles) {
    if (!savedFiles.length) return text
    let content = text
    for (const f of savedFiles) {
      if (f.isImage) {
        content += `\n[IMAGE:${f.path}]`
      } else {
        content += `\n[附件] 原始文件名: ${f.name} → 已保存到: ${f.path} (请使用此完整路径读取文件)`
      }
    }
    return content
  }

  return {
    attachedFiles,
    addFiles,
    removeFile,
    clearFiles,
    onFileSelect,
    onPaste,
    onDrop,
    onDragOver,
    formatFileSize,
    saveFilesToDisk,
    buildContentWithFiles,
  }
}
