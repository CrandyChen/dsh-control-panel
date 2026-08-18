import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// 不使用 StrictMode：避免 dev 下 mount 效果双触发导致启动期重复执行状态/环境检测。
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
