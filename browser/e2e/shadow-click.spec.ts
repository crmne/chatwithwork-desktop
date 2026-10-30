import { expect, test } from "@playwright/test";
import { launch, serve, type Setup } from "./harness.js";

// Like component-library consent buttons: the accessible button lives in
// a shadow tree, with its label supplied as slotted text from the host.
const PAGE = `<!doctype html><title>Consent controls</title>
<style>consent-button { display: block; margin: 24px; width: 240px; }</style>
<consent-button>Essential cookies</consent-button>
<consent-button closed>Closed component</consent-button>
<consent-button covered>Covered component</consent-button>
<div style="position:relative"><consent-button>Outside overlay</consent-button><div style="position:absolute;inset:0;background:white">Cover</div></div>
<p id="result">No action</p>
<script>
customElements.define('consent-button',class extends HTMLElement {
  connectedCallback() {
    const root=this.attachShadow({mode:this.hasAttribute('closed')?'closed':'open'});
    root.innerHTML='<style>button{padding:18px;width:240px} div{position:relative}.cover{position:absolute;inset:0;background:white}</style><div><button type="button"><slot></slot></button>'+(this.hasAttribute('covered')?'<div class="cover">Cover</div>':'')+'</div>';
    root.querySelector('button').addEventListener('click',()=>document.querySelector('#result').textContent=this.textContent);
  }
});
</script>`;

let setup: Setup;
let server: { close: () => void };
let origin: string;

test.beforeAll(async () => {
  ({ server, origin } = await serve({ "/": PAGE }));
  setup = await launch([origin]);
  await setup.daemon.attached();
  setup.daemon.setPolicy({ changes: true });
});

test.afterAll(async () => { await setup?.cleanup(); server?.close(); });

for (const [label, covered] of [["Essential cookies", false], ["Closed component", false], ["Covered component", true], ["Outside overlay", true]] as const) {
  test(`${covered ? "refuses" : "clicks"} ${label} in a background tab`, async () => {
    const opened = await setup.daemon.call("open", { url: origin });
    expect(opened.error).toBeUndefined();
    const tab = opened.result.tab;
    const before = await setup.daemon.call("read", { tab });
    const line = before.result.text.split("\n").find((line: string) => line.includes(`button "${label}"`));
    const ref = /\[ref=(e\d+)\]/.exec(line || "")?.[1];
    expect(ref).toBeTruthy();
    const clicked = await setup.daemon.call("click", { tab, ref, submits: false });
    const after = await setup.daemon.call("read", { tab });
    if (covered) {
      expect(clicked.error?.code).toBe("page_error");
      expect(after.result.text).toContain("No action");
    } else {
      expect(clicked.error).toBeUndefined();
      expect(after.result.text).not.toContain("No action");
      expect(after.result.text).toContain(`paragraph: ${label}`);
    }
  });
}
