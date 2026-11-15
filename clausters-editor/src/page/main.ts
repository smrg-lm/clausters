// The page's entry: the editor, as the desktop's entry mounts it
// (../main.ts), and the page's own file dialog beside it.

import { mount } from "svelte";
import App from "../App.svelte";
import "../app.css";
import "./page.css";
import { installTooltips } from "../lib/ui/tooltip";
import FileDialog from "./FileDialog.svelte";

installTooltips();

mount(FileDialog, { target: document.body });
export default mount(App, { target: document.getElementById("app")! });
