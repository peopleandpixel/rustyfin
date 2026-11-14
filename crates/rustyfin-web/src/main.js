// Rustyfin WebUI entry point — imports CSS and the translations
// app.js and api.js are loaded as global scripts via <script> tags in index.html
import './input.css';
import './i18n.js';
import './icons.js';

// Signal that i18n is ready so classic scripts (app.js) can wait for it
window.dispatchEvent(new CustomEvent('i18n-ready'));
