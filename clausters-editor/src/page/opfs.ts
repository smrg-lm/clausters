// The page's files: the origin private file system (OPFS), the storage a page
// has of its own. A path is absolute from its root, `/` separated
// (`/sketches/a.mjs`), as the desktop's paths are from the disk's. Files come
// into it by being created in the editor or uploaded, and leave it by being
// downloaded.

/** An entry of a folder. */
export interface Entry {
  name: string;
  /** Its absolute path. */
  path: string;
  folder: boolean;
}

const root = () => navigator.storage.getDirectory();

/** `path`'s parts, refusing what would leave the root. */
function parts(path: string): string[] {
  const out = path.split("/").filter((p) => p !== "" && p !== ".");
  if (out.some((p) => p === "..")) throw new Error(`Not a path in the page's storage: ${path}`);
  return out;
}

/** The absolute form of `path`. */
export const normalize = (path: string) => `/${parts(path).join("/")}`;

/** The folder `path` names, created on the way if `create`. */
async function folder(path: string, create = false): Promise<FileSystemDirectoryHandle> {
  let dir = await root();
  for (const name of parts(path)) dir = await dir.getDirectoryHandle(name, { create });
  return dir;
}

/** The folder holding `path`, and its last name. */
async function parent(path: string, create = false): Promise<[FileSystemDirectoryHandle, string]> {
  const names = parts(path);
  const name = names.pop();
  if (name === undefined) throw new Error("The root is a folder, not a file");
  return [await folder(names.join("/"), create), name];
}

export async function readText(path: string): Promise<string> {
  try {
    const [dir, name] = await parent(path);
    return await (await (await dir.getFileHandle(name)).getFile()).text();
  } catch {
    throw new Error(`Could not read ${path}: no such file in the page's storage`);
  }
}

export async function readBytes(path: string): Promise<Blob> {
  const [dir, name] = await parent(path);
  return (await dir.getFileHandle(name)).getFile();
}

/** Writes `contents` to `path`, creating the file and its folders. */
export async function write(path: string, contents: string | Blob): Promise<void> {
  const [dir, name] = await parent(path, true);
  const writable = await (await dir.getFileHandle(name, { create: true })).createWritable();
  await writable.write(contents);
  await writable.close();
}

export async function exists(path: string): Promise<boolean> {
  try {
    const [dir, name] = await parent(path);
    await dir.getFileHandle(name);
    return true;
  } catch {
    return false;
  }
}

/** A folder's entries, folders first, each group by name. */
export async function list(path: string): Promise<Entry[]> {
  const dir = await folder(path);
  const base = normalize(path);
  const out: Entry[] = [];
  for await (const [name, handle] of dir as unknown as AsyncIterable<[string, FileSystemHandle]>) {
    out.push({ name, path: `${base === "/" ? "" : base}/${name}`, folder: handle.kind === "directory" });
  }
  return out.sort((a, b) => Number(b.folder) - Number(a.folder) || a.name.localeCompare(b.name));
}

export async function makeFolder(path: string): Promise<void> {
  await folder(path, true);
}

/** Removes a file, or a folder and everything in it. */
export async function remove(path: string): Promise<void> {
  const [dir, name] = await parent(path);
  await dir.removeEntry(name, { recursive: true });
}

/** Hands the file at `path` to the browser as a download. */
export async function download(path: string): Promise<void> {
  const blob = await readBytes(path);
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = parts(path).pop() ?? "file";
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Asks the user for files from the disk and copies them into `folderPath`;
 *  returns the paths written. */
export function upload(folderPath: string): Promise<string[]> {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.onchange = async () => {
      try {
        const base = normalize(folderPath);
        const written: string[] = [];
        for (const file of input.files ?? []) {
          const path = `${base === "/" ? "" : base}/${file.name}`;
          await write(path, file);
          written.push(path);
        }
        resolve(written);
      } catch (error) {
        reject(error);
      }
    };
    input.oncancel = () => resolve([]);
    input.click();
  });
}
