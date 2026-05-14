// Shared helper for reading voice phrases from an assignment / expansion / quick
// action data blob. The canonical field is `voicePhrases: string[]`. Older configs
// store a single string under `voicePhrase` — keep reading that for one release
// cycle before dropping the fallback. Writes always go through the new field.

export function readVoicePhrases(data) {
  if (Array.isArray(data?.voicePhrases)) {
    return data.voicePhrases.filter(p => typeof p === 'string' && p.trim());
  }
  if (typeof data?.voicePhrase === 'string' && data.voicePhrase.trim()) {
    return [data.voicePhrase.trim()];
  }
  return [];
}

// Apply a voice-phrases array onto a `data` object for saving. If the cleaned
// list is empty, BOTH the new array and the legacy single-string field are
// deleted so no orphan keys linger in config. Otherwise the new array is
// written and the legacy field is dropped.
export function writeVoicePhrases(data, phrases) {
  const cleaned = (phrases || [])
    .map(p => (typeof p === 'string' ? p.trim() : ''))
    .filter(p => p.length > 0);
  if (cleaned.length === 0) {
    delete data.voicePhrases;
    delete data.voicePhrase;
  } else {
    data.voicePhrases = cleaned;
    delete data.voicePhrase;
  }
  return data;
}
