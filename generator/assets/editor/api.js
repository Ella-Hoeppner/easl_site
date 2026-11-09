// The site server's JSON API (see server/README.md), for the editor.
// Requests carry the session cookie; failures throw an `ApiError` with the
// server's message.

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

async function request(method, path, body) {
  const options = { method, headers: {}, credentials: "same-origin" };
  if (body instanceof Blob) {
    options.body = body;
    options.headers["Content-Type"] = body.type;
  } else if (body !== undefined) {
    options.body = JSON.stringify(body);
    options.headers["Content-Type"] = "application/json";
  }
  let response;
  try {
    response = await fetch(`/api${path}`, options);
  } catch {
    throw new ApiError(0, "Couldn't reach the server. Check your connection.");
  }
  if (response.status === 204) return null;
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    throw new ApiError(
      response.status,
      data?.error ?? `The server couldn't do that (${response.status}).`,
    );
  }
  return data;
}

const enc = encodeURIComponent;

export const api = {
  me: () => request("GET", "/me"),
  mySketches: (limit = 50) => request("GET", `/me/sketches?limit=${limit}`),
  myPlaylists: (sketchId) =>
    request("GET", `/me/playlists${sketchId ? `?sketch=${enc(sketchId)}` : ""}`),

  createSketch: (sketch) => request("POST", "/sketches", sketch),
  getSketch: (id) => request("GET", `/sketches/${enc(id)}`),
  updateSketch: (id, changes) => request("PUT", `/sketches/${enc(id)}`, changes),
  deleteSketch: (id) => request("DELETE", `/sketches/${enc(id)}`),
  forkSketch: (id, fork) => request("POST", `/sketches/${enc(id)}/fork`, fork),
  uploadThumbnail: (id, blob) => request("PUT", `/sketches/${enc(id)}/thumbnail`, blob),
  like: (id) => request("POST", `/sketches/${enc(id)}/like`),
  unlike: (id) => request("DELETE", `/sketches/${enc(id)}/like`),

  comments: (id) => request("GET", `/sketches/${enc(id)}/comments`),
  addComment: (id, body) => request("POST", `/sketches/${enc(id)}/comments`, { body }),
  deleteComment: (commentId) => request("DELETE", `/comments/${enc(commentId)}`),

  createPlaylist: (playlist) => request("POST", "/playlists", playlist),
  addToPlaylist: (playlistId, sketchId) =>
    request("POST", `/playlists/${enc(playlistId)}/items`, { sketch_id: sketchId }),
  removeFromPlaylist: (playlistId, sketchId) =>
    request("DELETE", `/playlists/${enc(playlistId)}/items/${enc(sketchId)}`),

  report: (kind, target, reason) => request("POST", "/reports", { kind, target, reason }),
};
