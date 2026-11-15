// Rustyfin WebUI entry point — imports CSS and the translations
// api.js is loaded as a global script via a <script> tag in index.html; app.js is the
// module entry of the client (modules/), bundled after this one
import './input.css';
import './i18n.js';
import './icons.js';

// Signal that i18n is ready so classic scripts can wait for it
window.dispatchEvent(new CustomEvent('i18n-ready'));
