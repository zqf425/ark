import { useMemo } from 'react'
import { useI18n } from '../../../shared/i18n'
import { diffLineCounts, parseUnifiedDiff } from './diff-lines'
import './diff-view.css'

/**
 * Line-level view of one file's unified patch. Pure presentation: parsing and
 * numbering live in `diff-lines.ts`, and binary/absent patches never reach
 * this component (the caller checks `file.patch` first).
 */
export function DiffView({ patch }: { patch: string }): JSX.Element {
  const { t } = useI18n()
  const lines = useMemo(() => parseUnifiedDiff(patch), [patch])
  const counts = useMemo(() => diffLineCounts(lines), [lines])

  return (
    <div className="ark-diff" role="figure" aria-label={t('workbench.diff.aria')}>
      <p className="ark-diff__summary">
        <span className="ark-diff__add">+{counts.additions}</span>{' '}
        <span className="ark-diff__del">−{counts.deletions}</span>
      </p>
      <table className="ark-diff__table">
        <tbody>
          {lines.map((line, index) => (
            <tr key={index} className={`ark-diff__line ark-diff__line--${line.type}`}>
              <td className="ark-diff__num">{line.oldNumber ?? ''}</td>
              <td className="ark-diff__num">{line.newNumber ?? ''}</td>
              <td className="ark-diff__cell">{line.text}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
