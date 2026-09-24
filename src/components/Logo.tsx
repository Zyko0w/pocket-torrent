// Logo de Pocket Torrent : même dessin que l'icône de l'application (src-tauri/icons/app-icon.svg),
// pour que la fenêtre, la barre des tâches et la zone de notification montrent la même chose.
export function Logo ({ size = 28 }: { size?: number }) {
  return (
    <svg className='logo' width={size} height={size} viewBox='0 0 1024 1024' aria-hidden='true'>
      <rect width='1024' height='1024' rx='232' fill='#8b5cf6' />
      <path d='M300 268H724Q772 268 772 316V548Q772 626 708 674L560 786Q512 822 464 786L316 674Q252 626 252 548V316Q252 268 300 268Z' fill='#fff' />
      <path d='M308 352H716' stroke='#8b5cf6' strokeWidth='18' strokeLinecap='round' strokeDasharray='34 30' opacity='0.55' />
      <path d='M512 432V632M420 548L512 640L604 548' fill='none' stroke='#8b5cf6' strokeWidth='68' strokeLinecap='round' strokeLinejoin='round' />
    </svg>
  )
}
