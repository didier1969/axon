# Architecture V2.1 : Native Backpressure & Traffic Shaping (TOC)

Ce diagramme décrit la mécanique "Mission-Critical" finale du flux de données Axon. Il prend en compte l'élimination de la double-persistance (le SQLite de Rust est supprimé) au profit d'une "Backpressure Naturelle" TCP/UDS gérée nativement par Elixir et Oban (qui garantit la survie aux crashs). Il intègre également les limites matérielles (CPU/RAM/IO) et le circuit breaker dynamique.

```mermaid
stateDiagram-v2
    direction TB

    %% -------------------------------------
    %% CONTROL PLANE (ELIXIR / OBAN) - THE ONLY SOURCE OF TRUTH
    %% -------------------------------------
    state "Elixir Watcher (Inotify)" as Watcher
    
    state "Oban DB (PostgreSQL/SQLite)\n[Garantie de Survie OS]" as Oban {
        state "Queue: indexing_hot (Priorité Utilisateur)" as HotQueue
        state "Queue: indexing_default (Scan Fond)" as NormalQueue
        state "Queue: indexing_titan (Fichiers Massifs >1MB)" as TitanQueue
        HotQueue --> NormalQueue : Preemption
    }
    
    Watcher --> HotQueue : User Edits
    Watcher --> NormalQueue : SCAN_ALL Command
    Watcher --> TitanQueue : File > 1MB

    %% -------------------------------------
    %% CIRCUIT BREAKER (RESOURCE MONITORING)
    %% -------------------------------------
    state "Resource Monitor (:os_mon)" as Monitor {
        state "CPU Check" as CPU
        state "RAM Check" as RAM
        state "IO Wait Check" as IO
    }
    
    state "Backpressure Controller\n(Dynamic Scaling)" as Controller
    Monitor --> Controller : Telemetry
    
    Controller --> Oban : Pause/Resume Queues\n(If Hard Limit Exceeded)
    Controller --> Oban : Scale Concurrency\n(Based on Pressure Ratio)

    state "Elixir Worker (Broadway / Oban)" as ElixirWorker {
        state "Pop Batch" as Pop
        state "Wait for Ack" as WaitAck
    }
    HotQueue --> Pop
    NormalQueue --> Pop
    TitanQueue --> Pop

    %% -------------------------------------
    %% LA FRONTIÈRE (TCP/UDS BACKPRESSURE)
    %% -------------------------------------
    state "Rust Bridge (UDS Socket)" as Bridge
    Pop --> Bridge : Push (if socket accepts)
    
    %% -------------------------------------
    %% DATA PLANE (RUST) - STATELESS & IN-MEMORY
    %% -------------------------------------
    state "In-Memory Bounded Channel\n(Max 500 tasks)" as RAMQueue
    
    Bridge --> RAMQueue : Insert (Bloque Elixir si plein)

    state "Worker Pool (14 Threads CPU)" as Workers {
        state "AST Parsing (TreeSitter)" as Parse
        state "Vector Embedding (ONNX)" as Embed
        Parse --> Embed
    }

    RAMQueue --> Workers : Claim (if tunnel < 1000)

    %% -------------------------------------
    %% THE CHOKE POINT (WRITER ACTOR)
    %% -------------------------------------
    state "Writer Actor (Single Thread)" as Writer {
        state "Check Semaphore (mcp_active > 0)" as CheckMCP
        state "try_write_for(100ms)" as TryLock
        
        CheckMCP --> TryLock : Semaphore Free (No Reader)
        CheckMCP --> Error_Busy : Reader Detected (Yield)
        
        TryLock --> KuzuDB : Lock Acquired -> insert_file_data()
        TryLock --> Error_Busy : Timeout (DB Locked)
        
        KuzuDB --> Success_Done : Insertion OK
        KuzuDB --> Error_Fatal : Cypher/Syntax Panic
    }

    Workers --> Writer : Push to 1000-slot Channel

    %% -------------------------------------
    %% LECTEURS (AGENTS IA) - PRIORITÉ ABSOLUE
    %% -------------------------------------
    state "Agent IA (HTTP MCP Server)" as Agent
    Agent --> KuzuDB : 🚨 try_read_for(100ms) \n [Allume le Sémaphore mcp_active]

    %% -------------------------------------
    %% ROUTAGE DU FEEDBACK (VERS ELIXIR)
    %% -------------------------------------
    Success_Done --> WaitAck : UDS Event (status: ok)
    
    state "Rejection Router (Rust -> Elixir)" as Rejection
    Error_Busy --> Rejection : "System Contention"
    Error_Fatal --> Rejection : "File Corruption"
    
    Rejection --> WaitAck : UDS Event (status: error/busy)

    %% -------------------------------------
    %% LA RÉSOLUTION (DANS ELIXIR/OBAN)
    %% -------------------------------------
    state WaitAck {
        state "If OK: Mark Done" as Done
        state "If Busy: Exponential Backoff\n(Oban Native Retry)" as Retry
        state "If Error > 3 times: Mark POISON" as Poison
    }
```

### Principes Architecturaux Clefs (TOC & BEAM)

1. **Éradication de la Double Persistance :** Rust est devenu un moteur "Stateless" pur calcul. La file d'attente sur disque n'existe que dans le Control Plane Elixir (Oban). Si l'OS crashe, Oban relancera automatiquement les travaux interrompus au redémarrage de la machine.
2. **Backpressure Naturelle (UDS) :** Si la base de Graphe bloque, les 14 Workers s'arrêtent, le canal en mémoire (`RAMQueue`) se remplit, et le socket Unix (UDS) arrête de lire. Elixir est contraint de ralentir naturellement son émission de tâches sans saturer la RAM.
3. **Traffic Shaping (Priorité au Read) :** La ligne de vie de l'IA (Agent) coupe systématiquement la route à l'indexation. Si `mcp_active` est allumé, le Writer Rust renvoie `Error_Busy`.
4. **Smart Retry externalisé :** C'est Oban (Elixir) qui gère l'Exponential Backoff. Un fichier ralenti par le trafic retournera dans la base SQL avec un délai mathématique avant sa prochaine tentative, garantissant la dilution parfaite de la charge au fil du temps. Les erreurs fatales (Corruptions) seront classées en POISON par le Dashboard Elixir.
5. **Circuit Breaker Dynamique (Hardware limits) :** Un `ResourceMonitor` OS-level scrute la RAM, le CPU et l'I/O Wait. Si les limites matérielles (ex: 70% RAM) sont atteintes, le `BackpressureController` ordonne à Oban de suspendre les envois ou de réduire l'échelle de concurrence pour éviter un crash OOM Linux.
6. **Titan Mode :** Les fichiers monstrueux (>1MB) sont déviés vers la file isolée `indexing_titan` (à concurrence unitaire) pour éviter la famine des threads CPU Rust sur les petits fichiers de la `indexing_hot`.