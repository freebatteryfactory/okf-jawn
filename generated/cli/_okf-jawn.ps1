
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'okf-jawn' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'okf-jawn'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'okf-jawn' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('-V', '-V ', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('--version', '--version', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('list_workspaces', 'list_workspaces', [CompletionResultType]::ParameterValue, 'List only workspaces visible to the authenticated principal.')
            [CompletionResult]::new('create_workspace', 'create_workspace', [CompletionResultType]::ParameterValue, 'Create a blank workspace; no example content is inserted.')
            [CompletionResult]::new('open_workspace', 'open_workspace', [CompletionResultType]::ParameterValue, 'Open an existing authorized workspace.')
            [CompletionResult]::new('update_workspace', 'update_workspace', [CompletionResultType]::ParameterValue, 'Update workspace metadata at the supplied base revision.')
            [CompletionResult]::new('archive_workspace', 'archive_workspace', [CompletionResultType]::ParameterValue, 'Archive without deleting historical content.')
            [CompletionResult]::new('export_workspace', 'export_workspace', [CompletionResultType]::ParameterValue, 'Build a portable export with resolvable referenced assets.')
            [CompletionResult]::new('export', 'export', [CompletionResultType]::ParameterValue, 'Build a portable export with resolvable referenced assets.')
            [CompletionResult]::new('backup_workspace', 'backup_workspace', [CompletionResultType]::ParameterValue, 'Back up content and durable application records.')
            [CompletionResult]::new('list_items', 'list_items', [CompletionResultType]::ParameterValue, 'List a folder with one-line descriptions at one resolved revision.')
            [CompletionResult]::new('ls', 'ls', [CompletionResultType]::ParameterValue, 'List a folder with one-line descriptions at one resolved revision.')
            [CompletionResult]::new('get_item', 'get_item', [CompletionResultType]::ParameterValue, 'Read the editable Markdown and preserved properties of an item.')
            [CompletionResult]::new('create_item', 'create_item', [CompletionResultType]::ParameterValue, 'Create authored content without modifying source bytes.')
            [CompletionResult]::new('update_item', 'update_item', [CompletionResultType]::ParameterValue, 'Save content with a base revision precondition; obsolete reviews do not transfer.')
            [CompletionResult]::new('move_item', 'move_item', [CompletionResultType]::ParameterValue, 'Move an item and rewrite references in one committed change.')
            [CompletionResult]::new('set_lifecycle', 'set_lifecycle', [CompletionResultType]::ParameterValue, 'Change lifecycle without approving any claim in the document.')
            [CompletionResult]::new('delete_item', 'delete_item', [CompletionResultType]::ParameterValue, 'Remove the current reference while retaining historical source objects.')
            [CompletionResult]::new('create_folder', 'create_folder', [CompletionResultType]::ParameterValue, 'Create a user-selected folder with a maintained index.')
            [CompletionResult]::new('list_types', 'list_types', [CompletionResultType]::ParameterValue, 'List built-in and user-defined OKF property schemas.')
            [CompletionResult]::new('set_type', 'set_type', [CompletionResultType]::ParameterValue, 'Persist a user-defined type without dropping extension properties.')
            [CompletionResult]::new('read_item', 'read_item', [CompletionResultType]::ParameterValue, 'Read an outline, Markdown, selected images, pages, or original. Resolve latest once; return exact citations and visible continuation.')
            [CompletionResult]::new('show', 'show', [CompletionResultType]::ParameterValue, 'Read an outline, Markdown, selected images, pages, or original. Resolve latest once; return exact citations and visible continuation.')
            [CompletionResult]::new('get_sources', 'get_sources', [CompletionResultType]::ParameterValue, 'Show supporting sources and occurrence-specific provenance.')
            [CompletionResult]::new('sources', 'sources', [CompletionResultType]::ParameterValue, 'Show supporting sources and occurrence-specific provenance.')
            [CompletionResult]::new('get_object', 'get_object', [CompletionResultType]::ParameterValue, 'Return a bounded binary block authorized through its source; knowing a hash never grants access.')
            [CompletionResult]::new('read_object', 'read_object', [CompletionResultType]::ParameterValue, 'Return a bounded binary block authorized through its source; knowing a hash never grants access.')
            [CompletionResult]::new('search_items', 'search_items', [CompletionResultType]::ParameterValue, 'Search authorized content and return cited snippets, not whole-document dumps.')
            [CompletionResult]::new('grep', 'grep', [CompletionResultType]::ParameterValue, 'Search authorized content and return cited snippets, not whole-document dumps.')
            [CompletionResult]::new('get_links', 'get_links', [CompletionResultType]::ParameterValue, 'Read incoming or outgoing references at the selected revision.')
            [CompletionResult]::new('links', 'links', [CompletionResultType]::ParameterValue, 'Read incoming or outgoing references at the selected revision.')
            [CompletionResult]::new('get_graph', 'get_graph', [CompletionResultType]::ParameterValue, 'Read a bounded graph projection using the same authorized source identities.')
            [CompletionResult]::new('log_items', 'log_items', [CompletionResultType]::ParameterValue, 'Read content snapshots; Git history is not the complete application event log.')
            [CompletionResult]::new('log', 'log', [CompletionResultType]::ParameterValue, 'Read content snapshots; Git history is not the complete application event log.')
            [CompletionResult]::new('timeline', 'timeline', [CompletionResultType]::ParameterValue, 'Read content snapshots; Git history is not the complete application event log.')
            [CompletionResult]::new('diff_items', 'diff_items', [CompletionResultType]::ParameterValue, 'Compare two explicit versions and retain both references.')
            [CompletionResult]::new('diff', 'diff', [CompletionResultType]::ParameterValue, 'Compare two explicit versions and retain both references.')
            [CompletionResult]::new('changes', 'changes', [CompletionResultType]::ParameterValue, 'Compare two explicit versions and retain both references.')
            [CompletionResult]::new('commit_items', 'commit_items', [CompletionResultType]::ParameterValue, 'Name a snapshot from saved drafts against an unchanged base.')
            [CompletionResult]::new('snapshot', 'snapshot', [CompletionResultType]::ParameterValue, 'Name a snapshot from saved drafts against an unchanged base.')
            [CompletionResult]::new('restore_items', 'restore_items', [CompletionResultType]::ParameterValue, 'Restore selected historical content as a new commit without rewriting history.')
            [CompletionResult]::new('rewind', 'rewind', [CompletionResultType]::ParameterValue, 'Restore selected historical content as a new commit without rewriting history.')
            [CompletionResult]::new('blame_item', 'blame_item', [CompletionResultType]::ParameterValue, 'Show which commit last changed each selected line, not the origin of each fact.')
            [CompletionResult]::new('blame', 'blame', [CompletionResultType]::ParameterValue, 'Show which commit last changed each selected line, not the origin of each fact.')
            [CompletionResult]::new('who', 'who', [CompletionResultType]::ParameterValue, 'Show which commit last changed each selected line, not the origin of each fact.')
            [CompletionResult]::new('open_proposal', 'open_proposal', [CompletionResultType]::ParameterValue, 'Create a suggested change set without merging or marking anything reviewed.')
            [CompletionResult]::new('propose', 'propose', [CompletionResultType]::ParameterValue, 'Create a suggested change set without merging or marking anything reviewed.')
            [CompletionResult]::new('list_proposals', 'list_proposals', [CompletionResultType]::ParameterValue, 'List visible suggested changes.')
            [CompletionResult]::new('get_proposal', 'get_proposal', [CompletionResultType]::ParameterValue, 'Read the exact proposed content and base revision.')
            [CompletionResult]::new('accept_proposal', 'accept_proposal', [CompletionResultType]::ParameterValue, 'Accept exactly the confirmed proposal against the displayed head; never an agent tool.')
            [CompletionResult]::new('approve', 'approve', [CompletionResultType]::ParameterValue, 'Accept exactly the confirmed proposal against the displayed head; never an agent tool.')
            [CompletionResult]::new('decline_proposal', 'decline_proposal', [CompletionResultType]::ParameterValue, 'Close a proposal without applying its content.')
            [CompletionResult]::new('decline', 'decline', [CompletionResultType]::ParameterValue, 'Close a proposal without applying its content.')
            [CompletionResult]::new('add_comment', 'add_comment', [CompletionResultType]::ParameterValue, 'Add discussion without certifying content.')
            [CompletionResult]::new('create_confirmation', 'create_confirmation', [CompletionResultType]::ParameterValue, 'Create a session-bound confirmation for explicit human action; issuance is not review.')
            [CompletionResult]::new('create_review', 'create_review', [CompletionResultType]::ParameterValue, 'Record exact revision and content reviewed by an explicit authorized action; never infer review from OAuth identity.')
            [CompletionResult]::new('verify', 'verify', [CompletionResultType]::ParameterValue, 'Record exact revision and content reviewed by an explicit authorized action; never infer review from OAuth identity.')
            [CompletionResult]::new('list_reviews', 'list_reviews', [CompletionResultType]::ParameterValue, 'Show review coverage and revisions without extending it to later edits.')
            [CompletionResult]::new('create_upload', 'create_upload', [CompletionResultType]::ParameterValue, 'Register a source occurrence before authenticated binary upload.')
            [CompletionResult]::new('complete_upload', 'complete_upload', [CompletionResultType]::ParameterValue, 'Check expected bytes and digest before conversion.')
            [CompletionResult]::new('start_import', 'start_import', [CompletionResultType]::ParameterValue, 'Durably register conversion and source creation, preserving originals.')
            [CompletionResult]::new('import', 'import', [CompletionResultType]::ParameterValue, 'Durably register conversion and source creation, preserving originals.')
            [CompletionResult]::new('get_job', 'get_job', [CompletionResultType]::ParameterValue, 'Read durable job progress; a receipt does not imply completion.')
            [CompletionResult]::new('list_jobs', 'list_jobs', [CompletionResultType]::ParameterValue, 'List conversion, export, and maintenance work.')
            [CompletionResult]::new('retry_job', 'retry_job', [CompletionResultType]::ParameterValue, 'Retry the same durable work identity without duplicating completed effects.')
            [CompletionResult]::new('cancel_job', 'cancel_job', [CompletionResultType]::ParameterValue, 'Cancel work while retaining completed uploads and recorded state.')
            [CompletionResult]::new('redigest_item', 'redigest_item', [CompletionResultType]::ParameterValue, 'Re-run extraction with declared settings; preserve prior digests and separate corrections.')
            [CompletionResult]::new('correct_digest', 'correct_digest', [CompletionResultType]::ParameterValue, 'Save a correction separately from generated extraction.')
            [CompletionResult]::new('get_rules', 'get_rules', [CompletionResultType]::ParameterValue, 'Read the same naming conventions used by the form, YAML editor, and import process.')
            [CompletionResult]::new('set_rules', 'set_rules', [CompletionResultType]::ParameterValue, 'Save explicit rules as versioned context.')
            [CompletionResult]::new('preview_names', 'preview_names', [CompletionResultType]::ParameterValue, 'Compute before and after naming changes without mutating content.')
            [CompletionResult]::new('apply_names', 'apply_names', [CompletionResultType]::ParameterValue, 'Apply the checked preview and rewrite references in one versioned action.')
            [CompletionResult]::new('get_attention', 'get_attention', [CompletionResultType]::ParameterValue, 'Show actionable maintenance observations; an empty result is not a safety certificate.')
            [CompletionResult]::new('attention', 'attention', [CompletionResultType]::ParameterValue, 'Show actionable maintenance observations; an empty result is not a safety certificate.')
            [CompletionResult]::new('rebuild_index', 'rebuild_index', [CompletionResultType]::ParameterValue, 'Rebuild only derived search and link data; preserve reviews and receipts.')
            [CompletionResult]::new('get_view', 'get_view', [CompletionResultType]::ParameterValue, 'Read a saved visual artifact and its precise source bindings.')
            [CompletionResult]::new('present_view', 'present_view', [CompletionResultType]::ParameterValue, 'Render a validated candidate using approved components and real source bindings; does not save or approve it.')
            [CompletionResult]::new('present', 'present', [CompletionResultType]::ParameterValue, 'Render a validated candidate using approved components and real source bindings; does not save or approve it.')
            [CompletionResult]::new('resolve_view', 'resolve_view', [CompletionResultType]::ParameterValue, 'Resolve pinned inputs or explicitly refresh a live view without silently modifying saved state.')
            [CompletionResult]::new('export_view', 'export_view', [CompletionResultType]::ParameterValue, 'Export the spec, source references, data table and rendering.')
            [CompletionResult]::new('get_catalog', 'get_catalog', [CompletionResultType]::ParameterValue, 'Describe the allowed visual components; the catalog never grants capabilities.')
            [CompletionResult]::new('catalog', 'catalog', [CompletionResultType]::ParameterValue, 'Describe the allowed visual components; the catalog never grants capabilities.')
            [CompletionResult]::new('get_receipt', 'get_receipt', [CompletionResultType]::ParameterValue, 'Inspect the exact sources returned through this server, not an external model internal state.')
            [CompletionResult]::new('list_events', 'list_events', [CompletionResultType]::ParameterValue, 'Read resumable notifications without treating them as authoritative document content.')
            [CompletionResult]::new('get_session', 'get_session', [CompletionResultType]::ParameterValue, 'Read the current authenticated principal without exposing credentials.')
            [CompletionResult]::new('create_connector', 'create_connector', [CompletionResultType]::ParameterValue, 'Issue a local MCP connector credential scoped to read, or read and propose; never review or approve. Local owner only; hosted agents use WorkOS Connect.')
            [CompletionResult]::new('list_connectors', 'list_connectors', [CompletionResultType]::ParameterValue, 'List local connector credentials and their scopes without secrets. Local owner only.')
            [CompletionResult]::new('revoke_connector', 'revoke_connector', [CompletionResultType]::ParameterValue, 'Revoke a local connector credential immediately. Local owner only.')
            [CompletionResult]::new('get_health', 'get_health', [CompletionResultType]::ParameterValue, 'Read process liveness, not feature qualification.')
            [CompletionResult]::new('get_readiness', 'get_readiness', [CompletionResultType]::ParameterValue, 'Read configured dependency readiness.')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'okf-jawn;list_workspaces' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_workspace' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;open_workspace' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;update_workspace' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;archive_workspace' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;export_workspace' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;export' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;backup_workspace' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_items' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;ls' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;update_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;move_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;set_lifecycle' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;delete_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_folder' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_types' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;set_type' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;read_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;show' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_sources' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;sources' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_object' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;read_object' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;search_items' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;grep' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_links' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;links' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_graph' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;log_items' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;log' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;timeline' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;diff_items' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;diff' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;changes' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;commit_items' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;snapshot' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;restore_items' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;rewind' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;blame_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;blame' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;who' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;open_proposal' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;propose' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_proposals' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_proposal' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;accept_proposal' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;approve' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;decline_proposal' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;decline' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;add_comment' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_confirmation' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_review' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;verify' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_reviews' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_upload' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;complete_upload' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;start_import' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;import' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_job' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_jobs' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;retry_job' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;cancel_job' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;redigest_item' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;correct_digest' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_rules' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;set_rules' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;preview_names' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;apply_names' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_attention' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;attention' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;rebuild_index' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_view' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;present_view' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;present' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;resolve_view' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;export_view' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_catalog' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;catalog' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_receipt' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_events' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_session' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;create_connector' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;list_connectors' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;revoke_connector' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_health' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;get_readiness' {
            [CompletionResult]::new('--server', '--server', [CompletionResultType]::ParameterName, 'server')
            [CompletionResult]::new('--json', '--json', [CompletionResultType]::ParameterName, 'JSON request, @file, or - for stdin')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help')
            break
        }
        'okf-jawn;help' {
            [CompletionResult]::new('list_workspaces', 'list_workspaces', [CompletionResultType]::ParameterValue, 'List only workspaces visible to the authenticated principal.')
            [CompletionResult]::new('create_workspace', 'create_workspace', [CompletionResultType]::ParameterValue, 'Create a blank workspace; no example content is inserted.')
            [CompletionResult]::new('open_workspace', 'open_workspace', [CompletionResultType]::ParameterValue, 'Open an existing authorized workspace.')
            [CompletionResult]::new('update_workspace', 'update_workspace', [CompletionResultType]::ParameterValue, 'Update workspace metadata at the supplied base revision.')
            [CompletionResult]::new('archive_workspace', 'archive_workspace', [CompletionResultType]::ParameterValue, 'Archive without deleting historical content.')
            [CompletionResult]::new('export_workspace', 'export_workspace', [CompletionResultType]::ParameterValue, 'Build a portable export with resolvable referenced assets.')
            [CompletionResult]::new('backup_workspace', 'backup_workspace', [CompletionResultType]::ParameterValue, 'Back up content and durable application records.')
            [CompletionResult]::new('list_items', 'list_items', [CompletionResultType]::ParameterValue, 'List a folder with one-line descriptions at one resolved revision.')
            [CompletionResult]::new('get_item', 'get_item', [CompletionResultType]::ParameterValue, 'Read the editable Markdown and preserved properties of an item.')
            [CompletionResult]::new('create_item', 'create_item', [CompletionResultType]::ParameterValue, 'Create authored content without modifying source bytes.')
            [CompletionResult]::new('update_item', 'update_item', [CompletionResultType]::ParameterValue, 'Save content with a base revision precondition; obsolete reviews do not transfer.')
            [CompletionResult]::new('move_item', 'move_item', [CompletionResultType]::ParameterValue, 'Move an item and rewrite references in one committed change.')
            [CompletionResult]::new('set_lifecycle', 'set_lifecycle', [CompletionResultType]::ParameterValue, 'Change lifecycle without approving any claim in the document.')
            [CompletionResult]::new('delete_item', 'delete_item', [CompletionResultType]::ParameterValue, 'Remove the current reference while retaining historical source objects.')
            [CompletionResult]::new('create_folder', 'create_folder', [CompletionResultType]::ParameterValue, 'Create a user-selected folder with a maintained index.')
            [CompletionResult]::new('list_types', 'list_types', [CompletionResultType]::ParameterValue, 'List built-in and user-defined OKF property schemas.')
            [CompletionResult]::new('set_type', 'set_type', [CompletionResultType]::ParameterValue, 'Persist a user-defined type without dropping extension properties.')
            [CompletionResult]::new('read_item', 'read_item', [CompletionResultType]::ParameterValue, 'Read an outline, Markdown, selected images, pages, or original. Resolve latest once; return exact citations and visible continuation.')
            [CompletionResult]::new('get_sources', 'get_sources', [CompletionResultType]::ParameterValue, 'Show supporting sources and occurrence-specific provenance.')
            [CompletionResult]::new('get_object', 'get_object', [CompletionResultType]::ParameterValue, 'Return a bounded binary block authorized through its source; knowing a hash never grants access.')
            [CompletionResult]::new('search_items', 'search_items', [CompletionResultType]::ParameterValue, 'Search authorized content and return cited snippets, not whole-document dumps.')
            [CompletionResult]::new('get_links', 'get_links', [CompletionResultType]::ParameterValue, 'Read incoming or outgoing references at the selected revision.')
            [CompletionResult]::new('get_graph', 'get_graph', [CompletionResultType]::ParameterValue, 'Read a bounded graph projection using the same authorized source identities.')
            [CompletionResult]::new('log_items', 'log_items', [CompletionResultType]::ParameterValue, 'Read content snapshots; Git history is not the complete application event log.')
            [CompletionResult]::new('diff_items', 'diff_items', [CompletionResultType]::ParameterValue, 'Compare two explicit versions and retain both references.')
            [CompletionResult]::new('commit_items', 'commit_items', [CompletionResultType]::ParameterValue, 'Name a snapshot from saved drafts against an unchanged base.')
            [CompletionResult]::new('restore_items', 'restore_items', [CompletionResultType]::ParameterValue, 'Restore selected historical content as a new commit without rewriting history.')
            [CompletionResult]::new('blame_item', 'blame_item', [CompletionResultType]::ParameterValue, 'Show which commit last changed each selected line, not the origin of each fact.')
            [CompletionResult]::new('open_proposal', 'open_proposal', [CompletionResultType]::ParameterValue, 'Create a suggested change set without merging or marking anything reviewed.')
            [CompletionResult]::new('list_proposals', 'list_proposals', [CompletionResultType]::ParameterValue, 'List visible suggested changes.')
            [CompletionResult]::new('get_proposal', 'get_proposal', [CompletionResultType]::ParameterValue, 'Read the exact proposed content and base revision.')
            [CompletionResult]::new('accept_proposal', 'accept_proposal', [CompletionResultType]::ParameterValue, 'Accept exactly the confirmed proposal against the displayed head; never an agent tool.')
            [CompletionResult]::new('decline_proposal', 'decline_proposal', [CompletionResultType]::ParameterValue, 'Close a proposal without applying its content.')
            [CompletionResult]::new('add_comment', 'add_comment', [CompletionResultType]::ParameterValue, 'Add discussion without certifying content.')
            [CompletionResult]::new('create_confirmation', 'create_confirmation', [CompletionResultType]::ParameterValue, 'Create a session-bound confirmation for explicit human action; issuance is not review.')
            [CompletionResult]::new('create_review', 'create_review', [CompletionResultType]::ParameterValue, 'Record exact revision and content reviewed by an explicit authorized action; never infer review from OAuth identity.')
            [CompletionResult]::new('list_reviews', 'list_reviews', [CompletionResultType]::ParameterValue, 'Show review coverage and revisions without extending it to later edits.')
            [CompletionResult]::new('create_upload', 'create_upload', [CompletionResultType]::ParameterValue, 'Register a source occurrence before authenticated binary upload.')
            [CompletionResult]::new('complete_upload', 'complete_upload', [CompletionResultType]::ParameterValue, 'Check expected bytes and digest before conversion.')
            [CompletionResult]::new('start_import', 'start_import', [CompletionResultType]::ParameterValue, 'Durably register conversion and source creation, preserving originals.')
            [CompletionResult]::new('get_job', 'get_job', [CompletionResultType]::ParameterValue, 'Read durable job progress; a receipt does not imply completion.')
            [CompletionResult]::new('list_jobs', 'list_jobs', [CompletionResultType]::ParameterValue, 'List conversion, export, and maintenance work.')
            [CompletionResult]::new('retry_job', 'retry_job', [CompletionResultType]::ParameterValue, 'Retry the same durable work identity without duplicating completed effects.')
            [CompletionResult]::new('cancel_job', 'cancel_job', [CompletionResultType]::ParameterValue, 'Cancel work while retaining completed uploads and recorded state.')
            [CompletionResult]::new('redigest_item', 'redigest_item', [CompletionResultType]::ParameterValue, 'Re-run extraction with declared settings; preserve prior digests and separate corrections.')
            [CompletionResult]::new('correct_digest', 'correct_digest', [CompletionResultType]::ParameterValue, 'Save a correction separately from generated extraction.')
            [CompletionResult]::new('get_rules', 'get_rules', [CompletionResultType]::ParameterValue, 'Read the same naming conventions used by the form, YAML editor, and import process.')
            [CompletionResult]::new('set_rules', 'set_rules', [CompletionResultType]::ParameterValue, 'Save explicit rules as versioned context.')
            [CompletionResult]::new('preview_names', 'preview_names', [CompletionResultType]::ParameterValue, 'Compute before and after naming changes without mutating content.')
            [CompletionResult]::new('apply_names', 'apply_names', [CompletionResultType]::ParameterValue, 'Apply the checked preview and rewrite references in one versioned action.')
            [CompletionResult]::new('get_attention', 'get_attention', [CompletionResultType]::ParameterValue, 'Show actionable maintenance observations; an empty result is not a safety certificate.')
            [CompletionResult]::new('rebuild_index', 'rebuild_index', [CompletionResultType]::ParameterValue, 'Rebuild only derived search and link data; preserve reviews and receipts.')
            [CompletionResult]::new('get_view', 'get_view', [CompletionResultType]::ParameterValue, 'Read a saved visual artifact and its precise source bindings.')
            [CompletionResult]::new('present_view', 'present_view', [CompletionResultType]::ParameterValue, 'Render a validated candidate using approved components and real source bindings; does not save or approve it.')
            [CompletionResult]::new('resolve_view', 'resolve_view', [CompletionResultType]::ParameterValue, 'Resolve pinned inputs or explicitly refresh a live view without silently modifying saved state.')
            [CompletionResult]::new('export_view', 'export_view', [CompletionResultType]::ParameterValue, 'Export the spec, source references, data table and rendering.')
            [CompletionResult]::new('get_catalog', 'get_catalog', [CompletionResultType]::ParameterValue, 'Describe the allowed visual components; the catalog never grants capabilities.')
            [CompletionResult]::new('get_receipt', 'get_receipt', [CompletionResultType]::ParameterValue, 'Inspect the exact sources returned through this server, not an external model internal state.')
            [CompletionResult]::new('list_events', 'list_events', [CompletionResultType]::ParameterValue, 'Read resumable notifications without treating them as authoritative document content.')
            [CompletionResult]::new('get_session', 'get_session', [CompletionResultType]::ParameterValue, 'Read the current authenticated principal without exposing credentials.')
            [CompletionResult]::new('create_connector', 'create_connector', [CompletionResultType]::ParameterValue, 'Issue a local MCP connector credential scoped to read, or read and propose; never review or approve. Local owner only; hosted agents use WorkOS Connect.')
            [CompletionResult]::new('list_connectors', 'list_connectors', [CompletionResultType]::ParameterValue, 'List local connector credentials and their scopes without secrets. Local owner only.')
            [CompletionResult]::new('revoke_connector', 'revoke_connector', [CompletionResultType]::ParameterValue, 'Revoke a local connector credential immediately. Local owner only.')
            [CompletionResult]::new('get_health', 'get_health', [CompletionResultType]::ParameterValue, 'Read process liveness, not feature qualification.')
            [CompletionResult]::new('get_readiness', 'get_readiness', [CompletionResultType]::ParameterValue, 'Read configured dependency readiness.')
            [CompletionResult]::new('help', 'help', [CompletionResultType]::ParameterValue, 'Print this message or the help of the given subcommand(s)')
            break
        }
        'okf-jawn;help;list_workspaces' {
            break
        }
        'okf-jawn;help;create_workspace' {
            break
        }
        'okf-jawn;help;open_workspace' {
            break
        }
        'okf-jawn;help;update_workspace' {
            break
        }
        'okf-jawn;help;archive_workspace' {
            break
        }
        'okf-jawn;help;export_workspace' {
            break
        }
        'okf-jawn;help;backup_workspace' {
            break
        }
        'okf-jawn;help;list_items' {
            break
        }
        'okf-jawn;help;get_item' {
            break
        }
        'okf-jawn;help;create_item' {
            break
        }
        'okf-jawn;help;update_item' {
            break
        }
        'okf-jawn;help;move_item' {
            break
        }
        'okf-jawn;help;set_lifecycle' {
            break
        }
        'okf-jawn;help;delete_item' {
            break
        }
        'okf-jawn;help;create_folder' {
            break
        }
        'okf-jawn;help;list_types' {
            break
        }
        'okf-jawn;help;set_type' {
            break
        }
        'okf-jawn;help;read_item' {
            break
        }
        'okf-jawn;help;get_sources' {
            break
        }
        'okf-jawn;help;get_object' {
            break
        }
        'okf-jawn;help;search_items' {
            break
        }
        'okf-jawn;help;get_links' {
            break
        }
        'okf-jawn;help;get_graph' {
            break
        }
        'okf-jawn;help;log_items' {
            break
        }
        'okf-jawn;help;diff_items' {
            break
        }
        'okf-jawn;help;commit_items' {
            break
        }
        'okf-jawn;help;restore_items' {
            break
        }
        'okf-jawn;help;blame_item' {
            break
        }
        'okf-jawn;help;open_proposal' {
            break
        }
        'okf-jawn;help;list_proposals' {
            break
        }
        'okf-jawn;help;get_proposal' {
            break
        }
        'okf-jawn;help;accept_proposal' {
            break
        }
        'okf-jawn;help;decline_proposal' {
            break
        }
        'okf-jawn;help;add_comment' {
            break
        }
        'okf-jawn;help;create_confirmation' {
            break
        }
        'okf-jawn;help;create_review' {
            break
        }
        'okf-jawn;help;list_reviews' {
            break
        }
        'okf-jawn;help;create_upload' {
            break
        }
        'okf-jawn;help;complete_upload' {
            break
        }
        'okf-jawn;help;start_import' {
            break
        }
        'okf-jawn;help;get_job' {
            break
        }
        'okf-jawn;help;list_jobs' {
            break
        }
        'okf-jawn;help;retry_job' {
            break
        }
        'okf-jawn;help;cancel_job' {
            break
        }
        'okf-jawn;help;redigest_item' {
            break
        }
        'okf-jawn;help;correct_digest' {
            break
        }
        'okf-jawn;help;get_rules' {
            break
        }
        'okf-jawn;help;set_rules' {
            break
        }
        'okf-jawn;help;preview_names' {
            break
        }
        'okf-jawn;help;apply_names' {
            break
        }
        'okf-jawn;help;get_attention' {
            break
        }
        'okf-jawn;help;rebuild_index' {
            break
        }
        'okf-jawn;help;get_view' {
            break
        }
        'okf-jawn;help;present_view' {
            break
        }
        'okf-jawn;help;resolve_view' {
            break
        }
        'okf-jawn;help;export_view' {
            break
        }
        'okf-jawn;help;get_catalog' {
            break
        }
        'okf-jawn;help;get_receipt' {
            break
        }
        'okf-jawn;help;list_events' {
            break
        }
        'okf-jawn;help;get_session' {
            break
        }
        'okf-jawn;help;create_connector' {
            break
        }
        'okf-jawn;help;list_connectors' {
            break
        }
        'okf-jawn;help;revoke_connector' {
            break
        }
        'okf-jawn;help;get_health' {
            break
        }
        'okf-jawn;help;get_readiness' {
            break
        }
        'okf-jawn;help;help' {
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
