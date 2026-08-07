//  KnownFormats.swift
//  The format vocabulary the hidden-formats picker offers.

/// The formats the server's scanners index, lowercase — mirrors
/// `omnibus_shared::KNOWN_LIBRARY_FORMATS` (shared/src/lib.rs). A UI option
/// list only, not a validation gate: the picker also renders any saved token
/// this set doesn't know, so it can always be un-hidden.
let KnownLibraryFormats: Set<String> = ["epub", "pdf", "cbz", "m4b", "m4a", "mp3"]
