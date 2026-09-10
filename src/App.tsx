import "./App.css";

const navigationItems = [
  "Visão geral",
  "Atividade",
  "Desempenho",
  "Armazenamento",
  "Inicialização e segurança",
];

function App() {
  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">P</span>
          <div>
            <h1>Pulse</h1>
            <p>Saúde do seu Windows</p>
          </div>
        </div>

        <nav aria-label="Navegação principal">
          <ul>
            {navigationItems.map((item, index) => (
              <li key={item}>
                <button className={index === 0 ? "nav-item active" : "nav-item"} type="button">
                  {item}
                </button>
              </li>
            ))}
          </ul>
        </nav>

        <p className="local-first">Dados locais · sem conta</p>
      </aside>

      <section className="content" aria-labelledby="overview-title">
        <p className="eyebrow">VISÃO GERAL</p>
        <h2 id="overview-title">Seu PC, explicado.</h2>
        <p className="intro">
          O Pulse vai reunir atividade, desempenho, espaço em disco e sinais que merecem atenção.
        </p>

        <section className="empty-state" aria-label="Estado inicial">
          <span aria-hidden="true">◌</span>
          <h3>Coleta ainda não iniciada</h3>
          <p>Os módulos serão ativados gradualmente, sempre com dados locais e explicações claras.</p>
        </section>
      </section>
    </main>
  );
}

export default App;
