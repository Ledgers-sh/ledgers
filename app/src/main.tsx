import { render } from 'preact';
import { App } from './app';
import './styles.css';

render(<App />, document.getElementById('root') as HTMLElement);

if (import.meta.env.PROD && 'serviceWorker' in navigator) {
  window.addEventListener('load', () => {
    navigator.serviceWorker.register('/sw.js').catch(() => {
      // Offline caching is an enhancement; the app works without it.
    });
  });
}
