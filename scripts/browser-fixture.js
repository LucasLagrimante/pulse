// Browser-only test data. Loaded manually through agent-browser, never by the app.
(async () => {
const { mockIPC } = await import('/node_modules/@tauri-apps/api/mocks.js');
const leaf = (name, size, dir = false) => ({ id: name, name, path: `C:\\PulseTest\\${name}`, sizeBytes: size, fileCount: dir ? 12 : 1, dirCount: 0, isDir: dir, children: [] });
const root = { ...leaf('PulseTest', 1200000000, true), children: [leaf('Projetos', 800000000, true), leaf('arquivo-grande.zip', 300000000), leaf('Documentos', 100000000, true)] };
mockIPC(async (cmd) => {
  if (cmd === 'get_system_overview') return { cpuPercent: 24, memoryUsedGb: 20.8, memoryTotalGb: 31.9, diskUsedGb: 860, diskTotalGb: 930, startupItemCount: 3, topProcesses: [{name: 'Processo de teste', pid: 123, memoryMb: 256}] };
  if (cmd === 'get_performance_details') return { gpu: {value: 'NVIDIA GeForce RTX 4070 SUPER'}, vramBytes: {value: 12000000000}, temperatureCelsius: {unavailableReason: 'Sensor não disponível neste teste'} };
  if (cmd === 'get_system_drives') return [{path: 'C:\\', label: 'Unidade de teste C:', totalBytes: 930000000000, freeBytes: 70000000000}];
  if (cmd === 'get_custom_scan_folders') return ['D:\\Projetos com um nome longo\\Arquivos de teste'];
  if (cmd === 'start_disk_scan') return root;
  if (cmd === 'get_current_window_label') return 'main';
  if (cmd === 'get_debug_mode') return true;
  if (cmd === 'get_startup_registration') return {enabled: false};
  return [];
});
return 'Browser test fixture installed';
})()
