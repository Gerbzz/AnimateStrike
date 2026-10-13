# AnimateStrike local control

The frozen source passed a production-editor control workflow on Windows. This standalone source layout still needs its own build and tests. The control check does not replace real mouse, keyboard, accessibility or full-window screenshot review.

Launch with `--control PORT --control-read-root ABSOLUTE_EXISTING_DIR --control-write-root ABSOLUTE_EXISTING_DIR` and a fresh 64-character hexadecimal `ANIMATESTRIKE_CONTROL_TOKEN` in the process environment. Without `--control`, no listener starts. The listener binds only `127.0.0.1`, accepts at most four clients, limits a request line to 64 KiB, and enforces an absolute 15-second connection lifetime for reads, writes and reply waiting. A UI action already dispatched before that deadline may still finish after the client disconnects. The token is required in every JSON-lines request and is never written to logs or replies. A malformed, HTTP-shaped, oversized or unauthenticated line closes that connection before an editor action executes.

Each line is a JSON object: `{"id":1,"method":"state","auth":"<fresh token>","params":{}}`. A response has the same `id`, `ok`, and either `result` or `error`. A timeout, oversized response or disconnected client can mean the operation completed without a delivered reply; inspect `state` and output paths before retrying. The UI thread applies up to eight requests per frame. All frame numbers are zero-based.

| Method | Parameters | Production editor action |
| --- | --- | --- |
| `state` | none | Read selected part/frame, pose, cel starts, history and status. |
| `part.select` | `part`: 0 root, 1 torso, 2 head | Select a named part. |
| `frame.set` | `frame`: 0–23 | Scrub the timeline. |
| `brush.stroke` | `part`: 1 or 2; `frame`; `points`: 1–256 local cel `[x,y]` pairs in 0–127; `color`: RGBA8; `radius`: 1–12; optional `erase` | Use the editor's begin/paint/commit stroke methods. A held cel splits at the frame. |
| `pose.key` | `part`, `frame`, `x`, `y`, `rotation`, `scale` | Set the same position/rotation/scale key as the inspector. |
| `edit.undo`, `edit.redo` | none | Use the toolbar history actions. |
| `file.save` | `path`: new `.animatestrike` path under write root | Save a new editable revision. |
| `file.open` | `path`: existing project under read root | Reopen; unsaved edits block this action. |
| `file.export` | `path`: new folder under write root | Export 24 PNG frames. |
| `canvas.capture` | `path`: new `.png` under write root | Save the selected frame's canvas RGBA PNG from the same CPU render as the preview. This excludes UI chrome and checkerboard. |

Read files are canonicalized beneath the read root. New files and folders require an existing canonical parent beneath the write root and reject an existing target. For a save/reopen test, choose roots that both contain the project output. A control Open first commits any in-progress paint stroke, then rejects the request if that made the current project dirty. This design assumes the caller also controls local filesystem mutations while an operation executes; paths are checked before use, but standard filesystem operations cannot eliminate a concurrent symlink replacement race. Do not expose the token to a web page or untrusted local process.
