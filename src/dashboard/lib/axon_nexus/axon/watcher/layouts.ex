defmodule Axon.Watcher.Layouts do
  use Phoenix.Component

  def root(assigns) do
    ~H"""
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="csrf-token" content="axon_no_csrf" />
        <title>AXON | Industrial Command Center</title>
        <link rel="preconnect" href="https://fonts.googleapis.com" />
        <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
        <link
          href="https://fonts.googleapis.com/css2?family=Poppins:wght@400;600;700&family=Open+Sans:wght@400;500&display=swap"
          rel="stylesheet"
        />
        <style>
          :root {
            --bg-deep: #050505;
            --bg-card: #0d0d0d;
            --neon-green: #00ff41;
            --neon-blue: #00f2ff;
            --neon-red: #ff003c;
            --text-main: #e0e0e0;
            --text-dim: #888888;
            --border: #1a1a1a;
          }
          body { 
            background: var(--bg-deep); 
            color: var(--text-main); 
            font-family: 'Open Sans', sans-serif; 
            margin: 0; 
            padding: 24px;
            letter-spacing: 0.02em;
          }
          h1, h2, h3 { font-family: 'Poppins', sans-serif; text-transform: uppercase; letter-spacing: 0.1em; }
          .container { max-width: 1400px; margin: 0 auto; }
          .header { 
            display: flex; justify-content: space-between; align-items: center; 
            border-bottom: 2px solid var(--border); padding-bottom: 24px; margin-bottom: 40px;
            box-shadow: 0 4px 20px rgba(0,0,0,0.5);
          }
          .logo { font-size: 1.8rem; font-weight: 700; color: var(--neon-green); text-shadow: 0 0 10px rgba(0,255,65,0.3); }
          .status-badge { padding: 6px 16px; border-radius: 4px; font-size: 0.75rem; font-weight: 700; border: 1px solid transparent; }
          .status-live { background: rgba(0,255,65,0.1); color: var(--neon-green); border-color: var(--neon-green); }
          .status-error { background: rgba(255,0,60,0.1); color: var(--neon-red); border-color: var(--neon-red); }
          .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(350px, 1fr)); gap: 24px; }
          .card { 
            background: var(--bg-card); 
            border: 1px solid var(--border); 
            border-radius: 4px; 
            padding: 24px;
            transition: all 0.3s ease;
            position: relative;
            overflow: hidden;
          }
          .card::before { content: ""; position: absolute; top: 0; left: 0; width: 100%; height: 2px; background: var(--border); transition: background 0.3s; }
          .card:hover { border-color: #333; box-shadow: 0 8px 30px rgba(0,0,0,0.6); }
          .card:hover::before { background: var(--neon-blue); }
          .card-title { font-size: 0.9rem; font-weight: 600; margin-bottom: 20px; color: var(--text-dim); display: flex; align-items: center; gap: 12px; }
          .card-title svg { color: var(--neon-blue); }
          .stat { margin-bottom: 12px; font-size: 0.9rem; display: flex; justify-content: space-between; }
          .stat label { color: var(--text-dim); }
          .stat span { color: #fff; font-weight: 600; font-family: monospace; }
          .progress-bar { height: 4px; background: #1a1a1a; border-radius: 2px; margin-top: 20px; }
          .progress-fill { height: 100%; background: var(--neon-green); box-shadow: 0 0 10px var(--neon-green); transition: width 0.5s cubic-bezier(0.4, 0, 0.2, 1); }
          .btn { 
            padding: 12px; border-radius: 4px; font-weight: 700; cursor: pointer; text-transform: uppercase; 
            font-size: 0.75rem; border: 1px solid var(--border); transition: all 0.2s;
            background: #111; color: #fff;
          }
          .btn-primary:hover { background: var(--neon-blue); color: #000; border-color: var(--neon-blue); box-shadow: 0 0 15px var(--neon-blue); }
          .btn-danger:hover { background: var(--neon-red); color: #fff; border-color: var(--neon-red); box-shadow: 0 0 15px var(--neon-red); }
          .pulse { width: 8px; height: 8px; border-radius: 50%; background: var(--neon-green); box-shadow: 0 0 8px var(--neon-green); animation: pulse 2s infinite; }
          @keyframes pulse { 0% { opacity: 1; transform: scale(1); } 50% { opacity: 0.4; transform: scale(1.2); } 100% { opacity: 1; transform: scale(1); } }
          ::-webkit-scrollbar { width: 6px; }
          ::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }
        </style>
      </head>
      <body>
        <div class="container">
          {@inner_content}
        </div>
        <script src="https://cdn.jsdelivr.net/npm/phoenix@1.8.5/priv/static/phoenix.min.js">
        </script>
        <script src="https://cdn.jsdelivr.net/npm/phoenix_live_view@1.0.18/priv/static/phoenix_live_view.min.js">
        </script>
        <script>
          window.addEventListener("load", () => {
            if (!window.liveSocket) {
              console.log("🔌 Connecting to Axon LiveView...");
              const {LiveSocket} = window.LiveView;
              window.liveSocket = new LiveSocket("/live", window.Phoenix.Socket, {
                params: {_csrf_token: "axon_no_csrf"}
              })
              window.liveSocket.connect()
            }
          });
        </script>
      </body>
    </html>
    """
  end
end
