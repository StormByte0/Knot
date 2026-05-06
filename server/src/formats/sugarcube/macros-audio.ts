/**
 * Knot v2 — SugarCube 2 Audio Macros
 *
 * Audio track management, playback control, groups, and playlists.
 * audio / cacheaudio / createaudiogroup / createplaylist /
 * masteraudio / playlist / removeaudiogroup / removeplaylist / waitforaudio
 */

import type { MacroDef } from '../_types';
import { mc, sig, arg, MacroKind } from './macros-helpers';

export function getAudioMacros(): MacroDef[] {
  return [
    mc('audio', 'audio', MacroKind.Command,
      'Control audio playback',
      [sig([arg('id', 'string', true, { description: 'Audio track ID' }), arg('action', 'string', false, { description: 'Action and arguments' })])],
    ),
    mc('cacheaudio', 'audio', MacroKind.Command,
      'Cache an audio file for later playback',
      [sig([arg('id', 'string', true, { description: 'Track ID' }), arg('url', 'string', true, { description: 'Audio URL' })])],
    ),
    mc('createaudiogroup', 'audio', MacroKind.Command,
      'Create a group of audio tracks',
      [sig([arg('ids', 'string', true, { description: 'Comma-separated track IDs' })])],
    ),
    mc('createplaylist', 'audio', MacroKind.Command,
      'Create an audio playlist',
      [sig([arg('id', 'string', true, { description: 'Playlist ID' }), arg('tracks', 'string', true, { description: 'Track list' })])],
    ),
    mc('masteraudio', 'audio', MacroKind.Command,
      'Control the master audio',
      [sig([arg('action', 'string', false, { description: 'Action and arguments' })])],
    ),
    mc('playlist', 'audio', MacroKind.Command,
      'Control playlist playback',
      [sig([arg('id', 'string', true, { description: 'Playlist ID' }), arg('action', 'string', false, { description: 'Action and arguments' })])],
    ),
    mc('removeaudiogroup', 'audio', MacroKind.Command,
      'Remove an audio group',
      [sig([arg('id', 'string', true, { description: 'Group ID' })])],
    ),
    mc('removeplaylist', 'audio', MacroKind.Command,
      'Remove a playlist',
      [sig([arg('id', 'string', true, { description: 'Playlist ID' })])],
    ),
    mc('waitforaudio', 'audio', MacroKind.Command,
      'Wait for audio to finish loading',
      [sig([arg('id', 'string', true, { description: 'Track ID' })])],
    ),
  ];
}
