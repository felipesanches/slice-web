/*
 * Remembering fonts the user has already opened, in IndexedDB.
 *
 * Adapted from `lib/js/local-font-storage.mjs` in TypeRoof by Font Bureau
 * (https://github.com/FontBureau/TypeRoof), used under the Apache License 2.0. The
 * structure is theirs: a promise-wrapped IndexedDB handle, one object store keyed by a
 * stable name, and the font's bytes kept in the record so that recalling one costs
 * nothing. Slice keeps the slicing settings alongside the bytes, evicts by least-recent
 * use, and speaks in plain objects because the caller is WebAssembly rather than
 * JavaScript.
 *
 * IndexedDB rather than localStorage because fonts are measured in megabytes and
 * localStorage is a few, and it stores strings. This stores the ArrayBuffer as it came.
 */

const DB_NAME = "SliceFonts";
const DB_VERSION = 1;
const STORE = "fonts";

/** Keep the store bounded: fonts run to tens of megabytes and quota is not ours. */
const MAX_FONTS = 12;
const MAX_TOTAL_BYTES = 120 * 1024 * 1024;
/** A single font larger than this is used but not remembered. */
const MAX_ONE_BYTES = 40 * 1024 * 1024;

function promised(request) {
    return new Promise((resolve, reject) => {
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
    });
}

let dbPromise = null;

function open() {
    if (dbPromise) return dbPromise;
    dbPromise = new Promise((resolve, reject) => {
        let request;
        try {
            request = indexedDB.open(DB_NAME, DB_VERSION);
        } catch (e) {
            // Firefox in private browsing throws here rather than failing the request.
            reject(e);
            return;
        }
        request.onupgradeneeded = () => {
            const db = request.result;
            if (!db.objectStoreNames.contains(STORE)) {
                db.createObjectStore(STORE, { keyPath: "id" });
            }
        };
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
        // An open blocked by another tab mid-upgrade would otherwise hang forever.
        request.onblocked = () => reject(new Error("blocked by another tab"));
    }).catch((e) => {
        // Remember the failure so every later call short-circuits instead of retrying a
        // database the browser has already refused.
        dbPromise = Promise.reject(e);
        throw e;
    });
    return dbPromise;
}

/**
 * Every remembered font, most recently used first, without the bytes.
 *
 * The bytes are deliberately left behind: the list is rendered on every load and a
 * caller that wanted 12 fonts' worth of ArrayBuffer to draw a menu would be paying
 * tens of megabytes for it.
 */
export async function listFonts() {
    let db;
    try {
        db = await open();
    } catch {
        return [];
    }
    const records = await promised(db.transaction(STORE, "readonly").objectStore(STORE).getAll());
    return records
        .map(({ id, name, family, settings, used, size }) => ({
            id,
            name,
            family,
            settings,
            used,
            size,
        }))
        .sort((a, b) => b.used - a.used);
}

/** One font's bytes, or null if it is no longer there. */
export async function getFont(id) {
    let db;
    try {
        db = await open();
    } catch {
        return null;
    }
    const record = await promised(db.transaction(STORE, "readonly").objectStore(STORE).get(id));
    return record ? new Uint8Array(record.buffer) : null;
}

/**
 * Remember a font and the settings last used with it.
 *
 * Returns true when it was stored. A refusal is not an error the caller should surface:
 * the font is already loaded and working, and "your browser would not let me remember
 * this" is not something to interrupt someone with.
 */
export async function putFont(id, name, family, bytes, settings) {
    if (bytes.byteLength > MAX_ONE_BYTES) return false;
    let db;
    try {
        db = await open();
    } catch {
        return false;
    }

    // Copy out of the wasm heap. The Uint8Array handed over is a view into linear memory,
    // which IndexedDB would serialise now and which the next allocation could move.
    const buffer = bytes.slice().buffer;

    try {
        const store = db.transaction(STORE, "readwrite").objectStore(STORE);
        await promised(
            store.put({
                id,
                name,
                family,
                settings,
                buffer,
                size: buffer.byteLength,
                used: Date.now(),
            }),
        );
    } catch {
        return false;
    }
    await evict();
    return true;
}

export async function forgetFont(id) {
    let db;
    try {
        db = await open();
    } catch {
        return;
    }
    try {
        await promised(db.transaction(STORE, "readwrite").objectStore(STORE).delete(id));
    } catch {
        /* nothing useful to do */
    }
}

export async function forgetAll() {
    let db;
    try {
        db = await open();
    } catch {
        return;
    }
    try {
        await promised(db.transaction(STORE, "readwrite").objectStore(STORE).clear());
    } catch {
        /* nothing useful to do */
    }
}

/** Drop the least recently used entries until the store is back inside its bounds. */
async function evict() {
    const db = await open();
    const records = await promised(db.transaction(STORE, "readonly").objectStore(STORE).getAll());
    records.sort((a, b) => b.used - a.used);

    let total = 0;
    const doomed = [];
    records.forEach((record, index) => {
        total += record.size || 0;
        if (index >= MAX_FONTS || total > MAX_TOTAL_BYTES) doomed.push(record.id);
    });
    if (!doomed.length) return;

    const store = db.transaction(STORE, "readwrite").objectStore(STORE);
    await Promise.all(doomed.map((id) => promised(store.delete(id))));
}
