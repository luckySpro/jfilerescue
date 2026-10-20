import { createApp } from "vue";
import App from "./App.vue";
import "./style.css";
import { effectiveLocale } from "./i18n";

document.documentElement.lang = effectiveLocale.value;
createApp(App).mount("#app");
