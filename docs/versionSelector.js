import {
  LitElement,
  css,
  html,
} from "https://cdn.jsdelivr.net/gh/lit/dist@3/all/lit-all.min.js";

class VersionSelector extends LitElement {
  static styles = css`
    select {
      padding: 5px;
      margin: 10px 0;
      font-size: 100%;
    }
  `;

  static properties = {
    versions: { type: Array },
  };

  constructor() {
    super();
    this.versions = [];
  }

  connectedCallback() {
    super.connectedCallback();
    this.fetchVersions();
  }

  async fetchVersions() {
    try {
      const response = await fetch(
        "https://martinvonz.github.io/jj/versions.json",
      );
      if (response.ok) {
        this.versions = await response.json();
      }
    } catch (error) {
      console.error("Error fetching versions:", error);
    }
  }

  handleVersionChange(e) {
    const selectedVersion = e.target.value;
    const currentUrl = new URL(window.location);
    const newUrl = currentUrl.href
      .replace(
        // /(\/jj\/)[^\/]+/,
        /http:\/\/localhost:3000(.*)/,
        `https://martinvonz.github.io/jj/${selectedVersion}$1`,
      )
      .replace(/\.html$/, "");
    window.location.href = newUrl;
  }

  render() {
    // TODO: show .aliases as well
    return html`
      <select @change="${this.handleVersionChange}">
        ${this.versions.map(
          (version) =>
            html`<option value="${version.version}">${version.title}</option>`,
        )}
      </select>
    `;
  }
}

customElements.define("version-selector", VersionSelector);
