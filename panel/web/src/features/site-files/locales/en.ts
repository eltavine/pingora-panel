import type { SiteFilesMessages } from './zh-CN'

const en: SiteFilesMessages = {
  siteFiles: {
    title: 'Site files',
    description: 'The files below the directory the gateway serves static sites from.',
    location: 'Location',
    root: 'Sites',
    name: 'Name',
    size: 'Size',
    modified: 'Modified',
    upload: 'Upload',
    uploaded: 'Uploaded {count} file | Uploaded {count} files',
    uploadFailed: 'Could not upload {name}',
    newFolder: 'New folder',
    folderName: 'Folder name',
    folderCreated: 'Created {name}',
    edit: 'Edit {name}',
    view: 'View {name}',
    download: 'Download {name}',
    downloadFailed: 'Could not download the file',
    remove: 'Remove {name}',
    removeTitle: 'Remove {name}?',
    removeDetail: 'It is gone for good, and the gateway stops serving it at once.',
    recursive: 'With everything in it',
    removed: 'Removed {name}',
    empty: 'This folder is empty',
    emptyDetail: 'Upload files here, or drop them on this card.',
    content: 'Content of {path}',
    editDetail: 'Saving replaces the file at once; the gateway serves it right away.',
    viewDetail: 'Viewing only: changing files needs the files.write permission.',
    saved: 'Saved {path}',
    stale: 'It changed since you opened it; reload it to see what is there now.',
    reload: 'Reload',
  },
}

export default en
