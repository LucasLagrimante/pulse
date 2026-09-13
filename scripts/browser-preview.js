// Isolated visual check for the exported tray component, in the browser only.
(async () => {
  const { default: React } = await import('/node_modules/.vite/deps/react.js');
  const { default: { createRoot } } = await import('/node_modules/.vite/deps/react-dom_client.js');
  const { TrayFlyout } = await import('/src/App.tsx');
  document.getElementById('root').style.display = 'none';
  const host = document.createElement('div');
  document.body.append(host);
  window.pulsePreviewRoot = createRoot(host);
  window.pulsePreviewRoot.render(React.createElement(TrayFlyout, { onOpenMain: () => {} }));
  return 'Tray preview mounted with browser test fixture';
})()
