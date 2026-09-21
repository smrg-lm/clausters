import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";
import { installTooltips } from "./lib/ui/tooltip";

installTooltips();

export default mount(App, { target: document.getElementById("app")! });
