import { mountInput, observeInputs } from './input'
import { mountSegmented } from './segmented'
import './uikit'
import './styles/islands.css'
// 外壳须先同步创建页面挂载点，业务岛才能接管各 section。
import './desktop-shell'
import './desktop-tooltip'
import './desktop-titlebar'
import './island-registry'

declare global {
  interface Window {
    wbSegmented?: { mount: typeof mountSegmented }
    wbInput?: { mount: typeof mountInput }
  }
}

window.wbSegmented = { mount: mountSegmented }
window.wbInput = { mount: mountInput }
observeInputs()
