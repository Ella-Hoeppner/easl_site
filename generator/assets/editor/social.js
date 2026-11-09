// The social side of the editor page: the open sketch's title, author,
// like button, and playlists in the bar, and the "About" and "Comments"
// tabs under the canvas (description, visibility, forking, reporting,
// deleting, and the comment thread).

import { $, el, fullDate, promptText, richText, timeAgo, toast } from "./util.js?v=__VERSION__";
import { api } from "./api.js?v=__VERSION__";

const VISIBILITIES = [
  ["private", "Private", "Only you can open it."],
  ["unlisted", "Unlisted", "Anyone with the link can open it; it isn't listed."],
  ["public", "Public", "Listed on Explore and your profile."],
];

export class Social {
  /**
   * `user` is the signed-in visitor (or null); `actions` has the editor's
   * document operations: `save()`, `fork()`, `renameDoc()`, `deleteDoc()`,
   * `updateThumbnail()`, and `signIn()` (the sign-in page, coming back
   * here after).
   */
  constructor(user, actions) {
    this.user = user;
    this.actions = actions;
    this.doc = null;
    this.commentsLoaded = null;
    for (const tab of document.querySelectorAll(".panel-tab")) {
      tab.addEventListener("click", () => this.selectTab(tab.dataset.panel));
    }
    $("like").addEventListener("click", () => this.toggleLike());
    $("playlist-button").addEventListener("click", () => this.togglePlaylists());
    $("sketch-title").addEventListener("click", () => {
      if (this.canRename()) this.actions.renameDoc();
    });
    document.addEventListener("pointerdown", (event) => {
      const popover = $("playlist-popover");
      if (!popover.hidden && !popover.contains(event.target) && !$("playlist-button").contains(event.target)) {
        popover.hidden = true;
      }
    });
  }

  canRename() {
    return this.doc?.canSave || this.doc?.kind === "new";
  }

  selectTab(name) {
    for (const tab of document.querySelectorAll(".panel-tab")) {
      const selected = tab.dataset.panel === name;
      tab.classList.toggle("active", selected);
      tab.setAttribute("aria-selected", String(selected));
    }
    for (const panel of document.querySelectorAll(".panel")) {
      panel.hidden = panel.id !== `${name}-panel`;
    }
    // Clearing, and the hint about the canvas, are about the console.
    $("clear").hidden = name !== "console";
    document.querySelector(".console-hint").hidden = name !== "console";
    if (name === "comments") this.loadComments();
  }

  /** Shows `doc`'s title, author, likes, details, and comments. */
  show(doc) {
    const changed = this.doc?.key !== doc.key;
    this.doc = doc;
    const sketch = doc.kind === "sketch" ? doc.sketch : null;
    const title = $("sketch-title");
    title.textContent = doc.title;
    title.disabled = !this.canRename();
    title.title = this.canRename() ? "Rename this sketch" : doc.title;
    $("byline").replaceChildren(...(sketch ? this.byline(sketch) : []));
    $("like").hidden = !sketch;
    $("playlist-button").hidden = !sketch;
    $("playlist-popover").hidden = true;
    $("comments-tab").hidden = !sketch;
    if (sketch) this.renderLike();
    this.renderAbout();
    if (changed) {
      this.commentsLoaded = null;
      $("comments-panel").replaceChildren();
      if (!sketch && document.querySelector(".panel-tab.active")?.dataset.panel === "comments") {
        this.selectTab("console");
      }
    }
    $("comments-count").textContent = sketch?.comment_count ? String(sketch.comment_count) : "";
  }

  // ---- likes ----

  renderLike() {
    const { liked, like_count } = this.doc.sketch;
    const button = $("like");
    button.classList.toggle("liked", liked);
    button.setAttribute("aria-pressed", String(liked));
    button.title = liked ? "Unlike this sketch" : "Like this sketch";
    $("like-label").textContent = liked ? "Liked" : "Like";
    $("like-count").textContent = String(like_count);
  }

  async toggleLike() {
    const sketch = this.doc?.sketch;
    if (!sketch) return;
    if (!this.user) {
      this.actions.signIn();
      return;
    }
    try {
      const result = sketch.liked ? await api.unlike(sketch.id) : await api.like(sketch.id);
      sketch.liked = result.liked;
      sketch.like_count = result.like_count;
      this.renderLike();
    } catch (error) {
      toast(error.message, "error");
    }
  }

  // ---- about ----

  renderAbout() {
    const doc = this.doc;
    const panel = $("about-panel");
    switch (doc.kind) {
      case "sketch":
        panel.replaceChildren(...this.aboutSketch(doc.sketch));
        break;
      default:
        panel.replaceChildren(
          el("h2", { className: "about-title" }, doc.title),
          el("p", { className: "about-note" },
            this.user
              ? "Not saved yet: your edits are kept in this browser until you save the sketch to your account."
              : "Not saved yet: your edits are kept in this browser. Sign in to save the sketch, publish it, and share it."),
          el("div", { className: "about-actions" },
            el("button", { className: "primary", onclick: () => this.actions.save() }, this.user ? "Save" : "Sign in to save"),
          ),
        );
    }
  }

  aboutSketch(sketch) {
    const owner = sketch.can_edit;
    const parts = [];
    parts.push(el("div", { className: "about-head" },
      el("h2", { className: "about-title" }, sketch.title),
      owner ? el("button", { className: "small", onclick: () => this.actions.renameDoc() }, "Rename") : null,
    ));
    parts.push(el("p", { className: "about-meta" },
      el("a", { href: `/u/${encodeURIComponent(sketch.owner.username)}`, className: "author" },
        el("img", { src: sketch.owner.avatar_url, alt: "", className: "avatar" }),
        sketch.owner.display_name || sketch.owner.username),
      " · ",
      el("span", { title: fullDate(sketch.created_at) }, `created ${timeAgo(sketch.created_at)}`),
      sketch.updated_at !== sketch.created_at
        ? [" · ", el("span", { title: fullDate(sketch.updated_at) }, `updated ${timeAgo(sketch.updated_at)}`)]
        : null,
    ));
    if (sketch.forked_from) {
      parts.push(el("p", { className: "about-meta" }, "Forked from ",
        el("a", { href: `/s/${encodeURIComponent(sketch.forked_from.id)}` }, sketch.forked_from.title),
        ` by @${sketch.forked_from.owner_username}`));
    }
    parts.push(el("p", { className: "about-stats" },
      `♥ ${sketch.like_count}`, " · ",
      `${sketch.view_count} view${sketch.view_count === 1 ? "" : "s"}`, " · ",
      `${sketch.comment_count} comment${sketch.comment_count === 1 ? "" : "s"}`));
    parts.push(owner ? this.descriptionEditor(sketch) : description(sketch.description));
    if (owner) parts.push(this.visibilityControl(sketch));
    else if (sketch.visibility !== "public") {
      parts.push(el("p", { className: "about-note" }, `This sketch is ${sketch.visibility}.`));
    }
    const actions = el("div", { className: "about-actions" });
    actions.append(el("button", { onclick: () => this.actions.fork() }, owner ? "Duplicate" : "Fork"));
    if (this.user && !owner) {
      actions.append(el("button", { className: "quiet", onclick: () => this.report("sketch", sketch.id) }, "Report"));
    }
    if (owner) {
      actions.append(
        el("button", {
          title: "Use what the canvas shows now as the sketch's picture on Explore and your profile",
          onclick: () => this.actions.updateThumbnail(),
        }, "Update thumbnail"),
        el("button", { className: "danger", onclick: () => this.actions.deleteDoc() }, "Delete"),
      );
    }
    parts.push(actions);
    return parts;
  }

  descriptionEditor(sketch) {
    const field = el("textarea", {
      className: "description-field",
      rows: 3,
      maxLength: 5000,
      placeholder: "Describe your sketch (what it shows, how to interact with it)…",
      value: sketch.description ?? "",
    });
    const status = el("span", { className: "field-status" });
    const save = el("button", { className: "small", disabled: true }, "Save description");
    field.addEventListener("input", () => {
      save.disabled = field.value === (sketch.description ?? "");
      status.textContent = "";
    });
    save.addEventListener("click", async () => {
      save.disabled = true;
      try {
        const updated = await api.updateSketch(sketch.id, { description: field.value });
        sketch.description = updated.description;
        sketch.updated_at = updated.updated_at;
        status.textContent = "Saved";
      } catch (error) {
        save.disabled = false;
        status.textContent = error.message;
      }
    });
    return el("div", { className: "description-editor" },
      el("label", { className: "field-label" }, "Description", field),
      el("div", { className: "field-row" }, save, status));
  }

  visibilityControl(sketch) {
    const status = el("span", { className: "field-status" });
    const options = VISIBILITIES.map(([value, label, help]) =>
      el("label", { className: `visibility-option${sketch.visibility === value ? " selected" : ""}`, title: help },
        el("input", {
          type: "radio",
          name: "visibility",
          value,
          checked: sketch.visibility === value,
          onchange: () => this.setVisibility(sketch, value),
        }),
        el("span", { className: "visibility-name" }, label),
        el("span", { className: "visibility-help" }, help)));
    return el("fieldset", { className: "visibility" },
      el("legend", { className: "field-label" }, "Visibility"), ...options, status);
  }

  /** Makes the visitor's own `sketch` private, unlisted, or public. */
  async setVisibility(sketch, value) {
    if (value === sketch.visibility) return;
    try {
      const updated = await api.updateSketch(sketch.id, { visibility: value });
      Object.assign(sketch, { visibility: updated.visibility, published_at: updated.published_at });
      toast(value === "public" ? "Published: it's on Explore now" : `It's ${value} now`);
    } catch (error) {
      toast(error.message, "error");
    }
    this.show(this.doc);
  }

  /** The bar's byline: the author, and the sketch's visibility (a button
   * opening a menu to change it, on the visitor's own sketch; for anyone
   * else, a badge on a sketch that isn't public). */
  byline(sketch) {
    const parts = [
      el("span", { className: "byline-by" }, "by"),
      el("a", { href: `/u/${encodeURIComponent(sketch.owner.username)}`, className: "author", title: `${sketch.owner.username}'s profile` },
        el("img", { src: sketch.owner.avatar_url, alt: "", className: "avatar" }),
        sketch.owner.display_name || sketch.owner.username),
    ];
    if (!sketch.can_edit) {
      if (sketch.visibility !== "public") {
        parts.push(el("span", { className: `badge ${sketch.visibility}` }, sketch.visibility));
      }
      return parts;
    }
    const menu = el("div", { className: "popover visibility-menu", role: "menu", hidden: true },
      el("p", { className: "popover-title" }, "Who can see it"),
      ...VISIBILITIES.map(([value, label, help]) =>
        el("button", {
          className: `visibility-choice${sketch.visibility === value ? " current" : ""}`,
          role: "menuitemradio",
          attrs: { "aria-checked": String(sketch.visibility === value) },
          onclick: () => {
            menu.hidden = true;
            this.setVisibility(sketch, value);
          },
        },
          el("span", { className: "visibility-name" }, label),
          el("span", { className: "visibility-help" }, help))));
    const button = el("button", {
      className: `badge badge-button ${sketch.visibility}`,
      title: "Change who can see this sketch",
      attrs: { "aria-haspopup": "menu" },
      onclick: () => {
        menu.hidden = !menu.hidden;
      },
    }, sketch.visibility, el("span", { className: "badge-caret", attrs: { "aria-hidden": "true" } }, "▾"));
    const anchor = el("span", { className: "popover-anchor" }, button, menu);
    const close = (event) => {
      if (!anchor.isConnected) document.removeEventListener("pointerdown", close);
      else if (!anchor.contains(event.target)) menu.hidden = true;
    };
    document.addEventListener("pointerdown", close);
    parts.push(anchor);
    return parts;
  }

  /** Opens (or closes) the popover listing the visitor's playlists, to add
   * the sketch to or take it out of, or to make a new one with it. */
  async togglePlaylists() {
    const sketch = this.doc?.sketch;
    const picker = $("playlist-popover");
    if (!sketch) return;
    if (!picker.hidden) {
      picker.hidden = true;
      return;
    }
    if (!this.user) {
      this.actions.signIn();
      return;
    }
    picker.hidden = false;
    picker.replaceChildren(el("p", { className: "about-note" }, "Loading your playlists…"));
    try {
      const { playlists } = await api.myPlaylists(sketch.id);
      const rows = playlists.map((playlist) =>
        el("label", { className: "playlist-row" },
          el("input", {
            type: "checkbox",
            checked: playlist.contains,
            onchange: async (event) => {
              const box = event.target;
              try {
                if (box.checked) await api.addToPlaylist(playlist.id, sketch.id);
                else await api.removeFromPlaylist(playlist.id, sketch.id);
              } catch (error) {
                box.checked = !box.checked;
                toast(error.message, "error");
              }
            },
          }),
          el("span", {}, playlist.title),
          el("a", { href: `/p/${encodeURIComponent(playlist.id)}`, className: "playlist-open" }, "open")));
      const create = el("button", {
        className: "small",
        onclick: async () => {
          const title = await promptText({
            title: "New playlist",
            label: "Name",
            confirm: "Create",
            validate: (value) => (value ? null : "A playlist needs a name."),
          });
          if (!title) return;
          try {
            await api.createPlaylist({ title, sketch_id: sketch.id });
            picker.hidden = true;
            this.togglePlaylists();
            toast(`Added to “${title}”`);
          } catch (error) {
            toast(error.message, "error");
          }
        },
      }, "New playlist…");
      picker.replaceChildren(
        el("p", { className: "popover-title" }, "Add to playlist"),
        ...(rows.length ? rows : [el("p", { className: "about-note" }, "You don't have any playlists yet.")]),
        create);
    } catch (error) {
      picker.replaceChildren(el("p", { className: "about-note error" }, error.message));
    }
  }

  async report(kind, target) {
    const reason = await promptText({
      title: "Report",
      label: "What's wrong with it?",
      confirm: "Report",
      validate: (value) => (value ? null : "Please say what's wrong."),
    });
    if (!reason) return;
    try {
      await api.report(kind, target, reason);
      toast("Thanks: the moderators will take a look.");
    } catch (error) {
      toast(error.message, "error");
    }
  }

  // ---- comments ----

  async loadComments() {
    const sketch = this.doc?.sketch;
    if (!sketch || this.commentsLoaded === sketch.id) return;
    this.commentsLoaded = sketch.id;
    const panel = $("comments-panel");
    panel.replaceChildren(el("p", { className: "about-note" }, "Loading comments…"));
    try {
      const { comments } = await api.comments(sketch.id);
      if (this.doc?.sketch?.id !== sketch.id) return;
      const list = el("ol", { className: "comments" }, ...comments.map((c) => this.comment(c)));
      panel.replaceChildren(list, this.commentForm(sketch, list));
      if (comments.length === 0) list.append(emptyComments());
    } catch (error) {
      this.commentsLoaded = null;
      panel.replaceChildren(el("p", { className: "about-note error" }, error.message));
    }
  }

  comment(comment) {
    const item = el("li", { className: "comment" },
      el("a", { href: `/u/${encodeURIComponent(comment.author.username)}`, className: "comment-avatar" },
        el("img", { src: comment.author.avatar_url, alt: "", className: "avatar" })),
      el("div", { className: "comment-main" },
        el("div", { className: "comment-head" },
          el("a", { href: `/u/${encodeURIComponent(comment.author.username)}`, className: "author" },
            comment.author.display_name || comment.author.username),
          el("span", { className: "comment-time", title: fullDate(comment.created_at) }, timeAgo(comment.created_at)),
          comment.can_delete
            ? el("button", {
                className: "comment-delete quiet small",
                onclick: async () => {
                  if (!confirm("Delete this comment?")) return;
                  try {
                    await api.deleteComment(comment.id);
                    item.remove();
                    this.adjustCommentCount(-1);
                  } catch (error) {
                    toast(error.message, "error");
                  }
                },
              }, "Delete")
            : this.user
              ? el("button", { className: "comment-delete quiet small", onclick: () => this.report("comment", String(comment.id)) }, "Report")
              : null),
        el("div", { className: "comment-body" }, ...richText(comment.body))));
    return item;
  }

  commentForm(sketch, list) {
    if (!this.user) {
      return el("p", { className: "comment-signin" },
        el("a", { href: "#", onclick: (event) => { event.preventDefault(); this.actions.signIn(); } }, "Sign in"),
        " to comment.");
    }
    if (!this.user.email_verified) {
      return el("p", { className: "comment-signin" },
        "Verify your email address to comment (see ", el("a", { href: "/settings" }, "settings"), ").");
    }
    const field = el("textarea", {
      rows: 2,
      maxLength: 2000,
      placeholder: "Add a comment…",
      className: "comment-field",
    });
    const post = el("button", { className: "primary small", type: "submit", disabled: true }, "Comment");
    field.addEventListener("input", () => (post.disabled = !field.value.trim()));
    field.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        event.stopPropagation();
        form.requestSubmit();
      }
    });
    const form = el("form", {
      className: "comment-form",
      onsubmit: async (event) => {
        event.preventDefault();
        const body = field.value.trim();
        if (!body) return;
        post.disabled = true;
        try {
          const comment = await api.addComment(sketch.id, body);
          list.querySelector(".comments-empty")?.remove();
          list.append(this.comment(comment));
          field.value = "";
          this.adjustCommentCount(1);
        } catch (error) {
          post.disabled = false;
          toast(error.message, "error");
        }
      },
    }, field, post);
    return form;
  }

  adjustCommentCount(change) {
    const sketch = this.doc?.sketch;
    if (!sketch) return;
    sketch.comment_count = Math.max(0, sketch.comment_count + change);
    $("comments-count").textContent = sketch.comment_count ? String(sketch.comment_count) : "";
  }
}

function description(text) {
  return text
    ? el("div", { className: "about-description" }, ...richText(text))
    : el("p", { className: "about-note" }, "No description.");
}

const emptyComments = () =>
  el("li", { className: "comments-empty" }, "No comments yet.");
