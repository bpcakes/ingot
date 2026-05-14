import { shortOid } from '../../lib/git'
import type { Convergence, ConvergenceConflict } from '../../types/domain'
import { DataTable } from '../DataTable'
import { StatusBadge } from '../StatusBadge'
import { TooltipValue } from '../TooltipValue'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '../ui/table'

const MAX_TOOLTIP_GIT_ERROR_CHARS = 240

function compactGitError(value: string) {
  const firstLine = value
    .split('\n')
    .find((line) => line.trim().length > 0)
    ?.trim()

  if (!firstLine) {
    return 'git cherry-pick failed'
  }

  const chars = Array.from(firstLine)
  return chars.length > MAX_TOOLTIP_GIT_ERROR_CHARS
    ? `${chars.slice(0, MAX_TOOLTIP_GIT_ERROR_CHARS).join('')}...`
    : firstLine
}

function hiddenConflictFileLabel(conflict: ConvergenceConflict, visibleCount: number) {
  const hiddenReturnedCount = Math.max(conflict.files.length - visibleCount, 0)
  const omittedCount = Math.max(conflict.total_file_count - conflict.files.length, 0)
  const parts = [
    hiddenReturnedCount ? `+${hiddenReturnedCount} more recorded` : null,
    omittedCount ? `+${omittedCount} not loaded` : null,
  ].filter((part): part is string => Boolean(part))

  return parts.length ? parts.join(', ') : null
}

export function ConvergencesTable({ convergences }: { convergences: Convergence[] }) {
  return (
    <DataTable title={`Convergences (${convergences.length})`}>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>ID</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Input target</TableHead>
            <TableHead>Prepared</TableHead>
            <TableHead>Final target</TableHead>
            <TableHead>Issue</TableHead>
            <TableHead>Valid</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {convergences.map((convergence) => {
            const isConflicted = convergence.status === 'conflicted'
            const conflictSummary = isConflicted ? convergence.conflict_summary : null
            const failureSummary = convergence.status === 'failed' ? convergence.failure_summary : null
            const conflict = isConflicted ? convergence.conflict : null
            const visibleConflictFiles = conflict?.files.slice(0, 3) ?? []
            const hiddenConflictFiles = conflict ? hiddenConflictFileLabel(conflict, visibleConflictFiles.length) : null
            const conflictFileLabel = conflict
              ? `${conflict.total_file_count} ${conflict.total_file_count === 1 ? 'file' : 'files'}${
                  conflict.files_truncated ? ', list truncated' : ''
                }`
              : null
            const conflictDetail = conflict
              ? [
                  `source commit: ${conflict.failed_source_commit_oid}`,
                  `files: ${conflictFileLabel}`,
                  `git error: ${compactGitError(conflict.git_error)}`,
                ].join('\n')
              : conflictSummary
            const issueSummary = conflictSummary ?? failureSummary

            return (
              <TableRow key={convergence.id}>
                <TableCell>
                  <code>{convergence.id}</code>
                </TableCell>
                <TableCell>
                  <StatusBadge status={convergence.status} />
                </TableCell>
                <TableCell>
                  <TooltipValue content={convergence.input_target_commit_oid}>
                    <code>{shortOid(convergence.input_target_commit_oid)}</code>
                  </TooltipValue>
                </TableCell>
                <TableCell>
                  <TooltipValue content={convergence.prepared_commit_oid}>
                    <code>{shortOid(convergence.prepared_commit_oid)}</code>
                  </TooltipValue>
                </TableCell>
                <TableCell>
                  <TooltipValue content={convergence.final_target_commit_oid}>
                    <code>{shortOid(convergence.final_target_commit_oid)}</code>
                  </TooltipValue>
                </TableCell>
                <TableCell className="max-w-[28rem]">
                  {issueSummary ? (
                    <div className="max-w-[28rem] space-y-1 whitespace-normal">
                      <TooltipValue content={issueSummary}>
                        <span className="block max-w-[28rem] truncate">{issueSummary}</span>
                      </TooltipValue>
                      {conflict ? (
                        <TooltipValue content={conflictDetail}>
                          <span className="block max-w-[28rem] truncate text-xs text-muted-foreground">
                            source <code>{shortOid(conflict.failed_source_commit_oid)}</code> - {conflictFileLabel}
                          </span>
                        </TooltipValue>
                      ) : null}
                      {visibleConflictFiles.length ? (
                        <div className="space-y-0.5 text-xs text-muted-foreground">
                          {visibleConflictFiles.map((file) => (
                            <TooltipValue
                              key={file.path}
                              content={file.excerpt ? `${file.path}\n\n${file.excerpt}` : file.path}
                            >
                              <span className="block max-w-[28rem] truncate">
                                <code>{file.path}</code>
                                {file.stages.length ? ` - ${file.stages.join('/')}` : ''}
                              </span>
                            </TooltipValue>
                          ))}
                          {hiddenConflictFiles ? <span className="block">{hiddenConflictFiles}</span> : null}
                        </div>
                      ) : null}
                    </div>
                  ) : (
                    <span className="text-muted-foreground">-</span>
                  )}
                </TableCell>
                <TableCell>{convergence.target_head_valid ? 'yes' : 'no'}</TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </DataTable>
  )
}
