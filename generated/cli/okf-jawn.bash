_okf__jawn() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="okf__jawn"
                ;;
            okf__jawn,accept_proposal)
                cmd="okf__jawn__subcmd__accept_proposal"
                ;;
            okf__jawn,add_comment)
                cmd="okf__jawn__subcmd__add_comment"
                ;;
            okf__jawn,apply_names)
                cmd="okf__jawn__subcmd__apply_names"
                ;;
            okf__jawn,approve)
                cmd="okf__jawn__subcmd__accept_proposal"
                ;;
            okf__jawn,archive_workspace)
                cmd="okf__jawn__subcmd__archive_workspace"
                ;;
            okf__jawn,attention)
                cmd="okf__jawn__subcmd__get_attention"
                ;;
            okf__jawn,backup_installation)
                cmd="okf__jawn__subcmd__backup_installation"
                ;;
            okf__jawn,backup_workspace)
                cmd="okf__jawn__subcmd__backup_workspace"
                ;;
            okf__jawn,blame)
                cmd="okf__jawn__subcmd__blame_item"
                ;;
            okf__jawn,blame_item)
                cmd="okf__jawn__subcmd__blame_item"
                ;;
            okf__jawn,cancel_job)
                cmd="okf__jawn__subcmd__cancel_job"
                ;;
            okf__jawn,catalog)
                cmd="okf__jawn__subcmd__get_catalog"
                ;;
            okf__jawn,changes)
                cmd="okf__jawn__subcmd__diff_items"
                ;;
            okf__jawn,commit_items)
                cmd="okf__jawn__subcmd__commit_items"
                ;;
            okf__jawn,complete_upload)
                cmd="okf__jawn__subcmd__complete_upload"
                ;;
            okf__jawn,correct_digest)
                cmd="okf__jawn__subcmd__correct_digest"
                ;;
            okf__jawn,create_confirmation)
                cmd="okf__jawn__subcmd__create_confirmation"
                ;;
            okf__jawn,create_connector)
                cmd="okf__jawn__subcmd__create_connector"
                ;;
            okf__jawn,create_folder)
                cmd="okf__jawn__subcmd__create_folder"
                ;;
            okf__jawn,create_item)
                cmd="okf__jawn__subcmd__create_item"
                ;;
            okf__jawn,create_review)
                cmd="okf__jawn__subcmd__create_review"
                ;;
            okf__jawn,create_sandbox_capability)
                cmd="okf__jawn__subcmd__create_sandbox_capability"
                ;;
            okf__jawn,create_upload)
                cmd="okf__jawn__subcmd__create_upload"
                ;;
            okf__jawn,create_workspace)
                cmd="okf__jawn__subcmd__create_workspace"
                ;;
            okf__jawn,decline)
                cmd="okf__jawn__subcmd__decline_proposal"
                ;;
            okf__jawn,decline_proposal)
                cmd="okf__jawn__subcmd__decline_proposal"
                ;;
            okf__jawn,delete_item)
                cmd="okf__jawn__subcmd__delete_item"
                ;;
            okf__jawn,diff)
                cmd="okf__jawn__subcmd__diff_items"
                ;;
            okf__jawn,diff_items)
                cmd="okf__jawn__subcmd__diff_items"
                ;;
            okf__jawn,discard_draft)
                cmd="okf__jawn__subcmd__discard_draft"
                ;;
            okf__jawn,export)
                cmd="okf__jawn__subcmd__export_workspace"
                ;;
            okf__jawn,export_view)
                cmd="okf__jawn__subcmd__export_view"
                ;;
            okf__jawn,export_workspace)
                cmd="okf__jawn__subcmd__export_workspace"
                ;;
            okf__jawn,get_attention)
                cmd="okf__jawn__subcmd__get_attention"
                ;;
            okf__jawn,get_catalog)
                cmd="okf__jawn__subcmd__get_catalog"
                ;;
            okf__jawn,get_graph)
                cmd="okf__jawn__subcmd__get_graph"
                ;;
            okf__jawn,get_health)
                cmd="okf__jawn__subcmd__get_health"
                ;;
            okf__jawn,get_item)
                cmd="okf__jawn__subcmd__get_item"
                ;;
            okf__jawn,get_job)
                cmd="okf__jawn__subcmd__get_job"
                ;;
            okf__jawn,get_links)
                cmd="okf__jawn__subcmd__get_links"
                ;;
            okf__jawn,get_object)
                cmd="okf__jawn__subcmd__get_object"
                ;;
            okf__jawn,get_proposal)
                cmd="okf__jawn__subcmd__get_proposal"
                ;;
            okf__jawn,get_purge)
                cmd="okf__jawn__subcmd__get_purge"
                ;;
            okf__jawn,get_readiness)
                cmd="okf__jawn__subcmd__get_readiness"
                ;;
            okf__jawn,get_receipt)
                cmd="okf__jawn__subcmd__get_receipt"
                ;;
            okf__jawn,get_rules)
                cmd="okf__jawn__subcmd__get_rules"
                ;;
            okf__jawn,get_session)
                cmd="okf__jawn__subcmd__get_session"
                ;;
            okf__jawn,get_sources)
                cmd="okf__jawn__subcmd__get_sources"
                ;;
            okf__jawn,get_tenant_job)
                cmd="okf__jawn__subcmd__get_tenant_job"
                ;;
            okf__jawn,get_view)
                cmd="okf__jawn__subcmd__get_view"
                ;;
            okf__jawn,grep)
                cmd="okf__jawn__subcmd__search_items"
                ;;
            okf__jawn,help)
                cmd="okf__jawn__subcmd__help"
                ;;
            okf__jawn,import)
                cmd="okf__jawn__subcmd__start_import"
                ;;
            okf__jawn,links)
                cmd="okf__jawn__subcmd__get_links"
                ;;
            okf__jawn,list_connectors)
                cmd="okf__jawn__subcmd__list_connectors"
                ;;
            okf__jawn,list_drafts)
                cmd="okf__jawn__subcmd__list_drafts"
                ;;
            okf__jawn,list_events)
                cmd="okf__jawn__subcmd__list_events"
                ;;
            okf__jawn,list_items)
                cmd="okf__jawn__subcmd__list_items"
                ;;
            okf__jawn,list_jobs)
                cmd="okf__jawn__subcmd__list_jobs"
                ;;
            okf__jawn,list_proposals)
                cmd="okf__jawn__subcmd__list_proposals"
                ;;
            okf__jawn,list_reviews)
                cmd="okf__jawn__subcmd__list_reviews"
                ;;
            okf__jawn,list_tenant_events)
                cmd="okf__jawn__subcmd__list_tenant_events"
                ;;
            okf__jawn,list_tenant_jobs)
                cmd="okf__jawn__subcmd__list_tenant_jobs"
                ;;
            okf__jawn,list_types)
                cmd="okf__jawn__subcmd__list_types"
                ;;
            okf__jawn,list_workspaces)
                cmd="okf__jawn__subcmd__list_workspaces"
                ;;
            okf__jawn,log)
                cmd="okf__jawn__subcmd__log_items"
                ;;
            okf__jawn,log_items)
                cmd="okf__jawn__subcmd__log_items"
                ;;
            okf__jawn,ls)
                cmd="okf__jawn__subcmd__list_items"
                ;;
            okf__jawn,move_item)
                cmd="okf__jawn__subcmd__move_item"
                ;;
            okf__jawn,open_proposal)
                cmd="okf__jawn__subcmd__open_proposal"
                ;;
            okf__jawn,open_workspace)
                cmd="okf__jawn__subcmd__open_workspace"
                ;;
            okf__jawn,present)
                cmd="okf__jawn__subcmd__present_view"
                ;;
            okf__jawn,present_view)
                cmd="okf__jawn__subcmd__present_view"
                ;;
            okf__jawn,preview_names)
                cmd="okf__jawn__subcmd__preview_names"
                ;;
            okf__jawn,propose)
                cmd="okf__jawn__subcmd__open_proposal"
                ;;
            okf__jawn,purge_item)
                cmd="okf__jawn__subcmd__purge_item"
                ;;
            okf__jawn,purge_workspace)
                cmd="okf__jawn__subcmd__purge_workspace"
                ;;
            okf__jawn,read_item)
                cmd="okf__jawn__subcmd__read_item"
                ;;
            okf__jawn,read_object)
                cmd="okf__jawn__subcmd__get_object"
                ;;
            okf__jawn,rebuild_index)
                cmd="okf__jawn__subcmd__rebuild_index"
                ;;
            okf__jawn,redigest_item)
                cmd="okf__jawn__subcmd__redigest_item"
                ;;
            okf__jawn,resolve_view)
                cmd="okf__jawn__subcmd__resolve_view"
                ;;
            okf__jawn,restore_items)
                cmd="okf__jawn__subcmd__restore_items"
                ;;
            okf__jawn,restore_workspace)
                cmd="okf__jawn__subcmd__restore_workspace"
                ;;
            okf__jawn,retry_job)
                cmd="okf__jawn__subcmd__retry_job"
                ;;
            okf__jawn,revoke_connector)
                cmd="okf__jawn__subcmd__revoke_connector"
                ;;
            okf__jawn,rewind)
                cmd="okf__jawn__subcmd__restore_items"
                ;;
            okf__jawn,save_draft)
                cmd="okf__jawn__subcmd__save_draft"
                ;;
            okf__jawn,search_items)
                cmd="okf__jawn__subcmd__search_items"
                ;;
            okf__jawn,set_lifecycle)
                cmd="okf__jawn__subcmd__set_lifecycle"
                ;;
            okf__jawn,set_rules)
                cmd="okf__jawn__subcmd__set_rules"
                ;;
            okf__jawn,set_type)
                cmd="okf__jawn__subcmd__set_type"
                ;;
            okf__jawn,show)
                cmd="okf__jawn__subcmd__read_item"
                ;;
            okf__jawn,snapshot)
                cmd="okf__jawn__subcmd__commit_items"
                ;;
            okf__jawn,sources)
                cmd="okf__jawn__subcmd__get_sources"
                ;;
            okf__jawn,start_import)
                cmd="okf__jawn__subcmd__start_import"
                ;;
            okf__jawn,timeline)
                cmd="okf__jawn__subcmd__log_items"
                ;;
            okf__jawn,unarchive_workspace)
                cmd="okf__jawn__subcmd__unarchive_workspace"
                ;;
            okf__jawn,update_workspace)
                cmd="okf__jawn__subcmd__update_workspace"
                ;;
            okf__jawn,verify)
                cmd="okf__jawn__subcmd__create_review"
                ;;
            okf__jawn,who)
                cmd="okf__jawn__subcmd__blame_item"
                ;;
            okf__jawn,workspaces)
                cmd="okf__jawn__subcmd__list_workspaces"
                ;;
            okf__jawn__subcmd__help,accept_proposal)
                cmd="okf__jawn__subcmd__help__subcmd__accept_proposal"
                ;;
            okf__jawn__subcmd__help,add_comment)
                cmd="okf__jawn__subcmd__help__subcmd__add_comment"
                ;;
            okf__jawn__subcmd__help,apply_names)
                cmd="okf__jawn__subcmd__help__subcmd__apply_names"
                ;;
            okf__jawn__subcmd__help,archive_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__archive_workspace"
                ;;
            okf__jawn__subcmd__help,backup_installation)
                cmd="okf__jawn__subcmd__help__subcmd__backup_installation"
                ;;
            okf__jawn__subcmd__help,backup_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__backup_workspace"
                ;;
            okf__jawn__subcmd__help,blame_item)
                cmd="okf__jawn__subcmd__help__subcmd__blame_item"
                ;;
            okf__jawn__subcmd__help,cancel_job)
                cmd="okf__jawn__subcmd__help__subcmd__cancel_job"
                ;;
            okf__jawn__subcmd__help,commit_items)
                cmd="okf__jawn__subcmd__help__subcmd__commit_items"
                ;;
            okf__jawn__subcmd__help,complete_upload)
                cmd="okf__jawn__subcmd__help__subcmd__complete_upload"
                ;;
            okf__jawn__subcmd__help,correct_digest)
                cmd="okf__jawn__subcmd__help__subcmd__correct_digest"
                ;;
            okf__jawn__subcmd__help,create_confirmation)
                cmd="okf__jawn__subcmd__help__subcmd__create_confirmation"
                ;;
            okf__jawn__subcmd__help,create_connector)
                cmd="okf__jawn__subcmd__help__subcmd__create_connector"
                ;;
            okf__jawn__subcmd__help,create_folder)
                cmd="okf__jawn__subcmd__help__subcmd__create_folder"
                ;;
            okf__jawn__subcmd__help,create_item)
                cmd="okf__jawn__subcmd__help__subcmd__create_item"
                ;;
            okf__jawn__subcmd__help,create_review)
                cmd="okf__jawn__subcmd__help__subcmd__create_review"
                ;;
            okf__jawn__subcmd__help,create_sandbox_capability)
                cmd="okf__jawn__subcmd__help__subcmd__create_sandbox_capability"
                ;;
            okf__jawn__subcmd__help,create_upload)
                cmd="okf__jawn__subcmd__help__subcmd__create_upload"
                ;;
            okf__jawn__subcmd__help,create_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__create_workspace"
                ;;
            okf__jawn__subcmd__help,decline_proposal)
                cmd="okf__jawn__subcmd__help__subcmd__decline_proposal"
                ;;
            okf__jawn__subcmd__help,delete_item)
                cmd="okf__jawn__subcmd__help__subcmd__delete_item"
                ;;
            okf__jawn__subcmd__help,diff_items)
                cmd="okf__jawn__subcmd__help__subcmd__diff_items"
                ;;
            okf__jawn__subcmd__help,discard_draft)
                cmd="okf__jawn__subcmd__help__subcmd__discard_draft"
                ;;
            okf__jawn__subcmd__help,export_view)
                cmd="okf__jawn__subcmd__help__subcmd__export_view"
                ;;
            okf__jawn__subcmd__help,export_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__export_workspace"
                ;;
            okf__jawn__subcmd__help,get_attention)
                cmd="okf__jawn__subcmd__help__subcmd__get_attention"
                ;;
            okf__jawn__subcmd__help,get_catalog)
                cmd="okf__jawn__subcmd__help__subcmd__get_catalog"
                ;;
            okf__jawn__subcmd__help,get_graph)
                cmd="okf__jawn__subcmd__help__subcmd__get_graph"
                ;;
            okf__jawn__subcmd__help,get_health)
                cmd="okf__jawn__subcmd__help__subcmd__get_health"
                ;;
            okf__jawn__subcmd__help,get_item)
                cmd="okf__jawn__subcmd__help__subcmd__get_item"
                ;;
            okf__jawn__subcmd__help,get_job)
                cmd="okf__jawn__subcmd__help__subcmd__get_job"
                ;;
            okf__jawn__subcmd__help,get_links)
                cmd="okf__jawn__subcmd__help__subcmd__get_links"
                ;;
            okf__jawn__subcmd__help,get_object)
                cmd="okf__jawn__subcmd__help__subcmd__get_object"
                ;;
            okf__jawn__subcmd__help,get_proposal)
                cmd="okf__jawn__subcmd__help__subcmd__get_proposal"
                ;;
            okf__jawn__subcmd__help,get_purge)
                cmd="okf__jawn__subcmd__help__subcmd__get_purge"
                ;;
            okf__jawn__subcmd__help,get_readiness)
                cmd="okf__jawn__subcmd__help__subcmd__get_readiness"
                ;;
            okf__jawn__subcmd__help,get_receipt)
                cmd="okf__jawn__subcmd__help__subcmd__get_receipt"
                ;;
            okf__jawn__subcmd__help,get_rules)
                cmd="okf__jawn__subcmd__help__subcmd__get_rules"
                ;;
            okf__jawn__subcmd__help,get_session)
                cmd="okf__jawn__subcmd__help__subcmd__get_session"
                ;;
            okf__jawn__subcmd__help,get_sources)
                cmd="okf__jawn__subcmd__help__subcmd__get_sources"
                ;;
            okf__jawn__subcmd__help,get_tenant_job)
                cmd="okf__jawn__subcmd__help__subcmd__get_tenant_job"
                ;;
            okf__jawn__subcmd__help,get_view)
                cmd="okf__jawn__subcmd__help__subcmd__get_view"
                ;;
            okf__jawn__subcmd__help,help)
                cmd="okf__jawn__subcmd__help__subcmd__help"
                ;;
            okf__jawn__subcmd__help,list_connectors)
                cmd="okf__jawn__subcmd__help__subcmd__list_connectors"
                ;;
            okf__jawn__subcmd__help,list_drafts)
                cmd="okf__jawn__subcmd__help__subcmd__list_drafts"
                ;;
            okf__jawn__subcmd__help,list_events)
                cmd="okf__jawn__subcmd__help__subcmd__list_events"
                ;;
            okf__jawn__subcmd__help,list_items)
                cmd="okf__jawn__subcmd__help__subcmd__list_items"
                ;;
            okf__jawn__subcmd__help,list_jobs)
                cmd="okf__jawn__subcmd__help__subcmd__list_jobs"
                ;;
            okf__jawn__subcmd__help,list_proposals)
                cmd="okf__jawn__subcmd__help__subcmd__list_proposals"
                ;;
            okf__jawn__subcmd__help,list_reviews)
                cmd="okf__jawn__subcmd__help__subcmd__list_reviews"
                ;;
            okf__jawn__subcmd__help,list_tenant_events)
                cmd="okf__jawn__subcmd__help__subcmd__list_tenant_events"
                ;;
            okf__jawn__subcmd__help,list_tenant_jobs)
                cmd="okf__jawn__subcmd__help__subcmd__list_tenant_jobs"
                ;;
            okf__jawn__subcmd__help,list_types)
                cmd="okf__jawn__subcmd__help__subcmd__list_types"
                ;;
            okf__jawn__subcmd__help,list_workspaces)
                cmd="okf__jawn__subcmd__help__subcmd__list_workspaces"
                ;;
            okf__jawn__subcmd__help,log_items)
                cmd="okf__jawn__subcmd__help__subcmd__log_items"
                ;;
            okf__jawn__subcmd__help,move_item)
                cmd="okf__jawn__subcmd__help__subcmd__move_item"
                ;;
            okf__jawn__subcmd__help,open_proposal)
                cmd="okf__jawn__subcmd__help__subcmd__open_proposal"
                ;;
            okf__jawn__subcmd__help,open_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__open_workspace"
                ;;
            okf__jawn__subcmd__help,present_view)
                cmd="okf__jawn__subcmd__help__subcmd__present_view"
                ;;
            okf__jawn__subcmd__help,preview_names)
                cmd="okf__jawn__subcmd__help__subcmd__preview_names"
                ;;
            okf__jawn__subcmd__help,purge_item)
                cmd="okf__jawn__subcmd__help__subcmd__purge_item"
                ;;
            okf__jawn__subcmd__help,purge_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__purge_workspace"
                ;;
            okf__jawn__subcmd__help,read_item)
                cmd="okf__jawn__subcmd__help__subcmd__read_item"
                ;;
            okf__jawn__subcmd__help,rebuild_index)
                cmd="okf__jawn__subcmd__help__subcmd__rebuild_index"
                ;;
            okf__jawn__subcmd__help,redigest_item)
                cmd="okf__jawn__subcmd__help__subcmd__redigest_item"
                ;;
            okf__jawn__subcmd__help,resolve_view)
                cmd="okf__jawn__subcmd__help__subcmd__resolve_view"
                ;;
            okf__jawn__subcmd__help,restore_items)
                cmd="okf__jawn__subcmd__help__subcmd__restore_items"
                ;;
            okf__jawn__subcmd__help,restore_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__restore_workspace"
                ;;
            okf__jawn__subcmd__help,retry_job)
                cmd="okf__jawn__subcmd__help__subcmd__retry_job"
                ;;
            okf__jawn__subcmd__help,revoke_connector)
                cmd="okf__jawn__subcmd__help__subcmd__revoke_connector"
                ;;
            okf__jawn__subcmd__help,save_draft)
                cmd="okf__jawn__subcmd__help__subcmd__save_draft"
                ;;
            okf__jawn__subcmd__help,search_items)
                cmd="okf__jawn__subcmd__help__subcmd__search_items"
                ;;
            okf__jawn__subcmd__help,set_lifecycle)
                cmd="okf__jawn__subcmd__help__subcmd__set_lifecycle"
                ;;
            okf__jawn__subcmd__help,set_rules)
                cmd="okf__jawn__subcmd__help__subcmd__set_rules"
                ;;
            okf__jawn__subcmd__help,set_type)
                cmd="okf__jawn__subcmd__help__subcmd__set_type"
                ;;
            okf__jawn__subcmd__help,start_import)
                cmd="okf__jawn__subcmd__help__subcmd__start_import"
                ;;
            okf__jawn__subcmd__help,unarchive_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__unarchive_workspace"
                ;;
            okf__jawn__subcmd__help,update_workspace)
                cmd="okf__jawn__subcmd__help__subcmd__update_workspace"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        okf__jawn)
            opts="-h -V --server --json --help --version list_workspaces workspaces create_workspace open_workspace update_workspace archive_workspace unarchive_workspace purge_workspace get_purge export_workspace export backup_workspace restore_workspace backup_installation list_items ls get_item create_item save_draft list_drafts discard_draft move_item set_lifecycle delete_item purge_item create_folder list_types set_type read_item show get_sources sources get_object read_object create_sandbox_capability search_items grep get_links links get_graph log_items log timeline diff_items diff changes commit_items snapshot restore_items rewind blame_item blame who open_proposal propose list_proposals get_proposal accept_proposal approve decline_proposal decline add_comment create_confirmation create_review verify list_reviews create_upload complete_upload start_import import get_job list_jobs get_tenant_job list_tenant_jobs retry_job cancel_job redigest_item correct_digest get_rules set_rules preview_names apply_names get_attention attention rebuild_index get_view present_view present resolve_view export_view get_catalog catalog get_receipt list_events list_tenant_events get_session create_connector list_connectors revoke_connector get_health get_readiness help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__accept_proposal)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__add_comment)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__apply_names)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__archive_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__backup_installation)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__backup_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__blame_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__cancel_job)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__commit_items)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__complete_upload)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__correct_digest)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_confirmation)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_connector)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_folder)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_review)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_sandbox_capability)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_upload)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__create_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__decline_proposal)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__delete_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__diff_items)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__discard_draft)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__export_view)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__export_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_attention)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_catalog)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_graph)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_health)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_job)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_links)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_object)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_proposal)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_purge)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_readiness)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_receipt)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_rules)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_session)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_sources)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_tenant_job)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__get_view)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help)
            opts="list_workspaces create_workspace open_workspace update_workspace archive_workspace unarchive_workspace purge_workspace get_purge export_workspace backup_workspace restore_workspace backup_installation list_items get_item create_item save_draft list_drafts discard_draft move_item set_lifecycle delete_item purge_item create_folder list_types set_type read_item get_sources get_object create_sandbox_capability search_items get_links get_graph log_items diff_items commit_items restore_items blame_item open_proposal list_proposals get_proposal accept_proposal decline_proposal add_comment create_confirmation create_review list_reviews create_upload complete_upload start_import get_job list_jobs get_tenant_job list_tenant_jobs retry_job cancel_job redigest_item correct_digest get_rules set_rules preview_names apply_names get_attention rebuild_index get_view present_view resolve_view export_view get_catalog get_receipt list_events list_tenant_events get_session create_connector list_connectors revoke_connector get_health get_readiness help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__accept_proposal)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__add_comment)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__apply_names)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__archive_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__backup_installation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__backup_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__blame_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__cancel_job)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__commit_items)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__complete_upload)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__correct_digest)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_confirmation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_connector)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_folder)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_review)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_sandbox_capability)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_upload)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__create_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__decline_proposal)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__delete_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__diff_items)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__discard_draft)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__export_view)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__export_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_attention)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_catalog)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_graph)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_health)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_job)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_links)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_object)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_proposal)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_purge)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_readiness)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_receipt)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_rules)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_session)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_sources)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_tenant_job)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__get_view)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_connectors)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_drafts)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_events)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_items)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_jobs)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_proposals)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_reviews)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_tenant_events)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_tenant_jobs)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_types)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__list_workspaces)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__log_items)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__move_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__open_proposal)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__open_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__present_view)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__preview_names)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__purge_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__purge_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__read_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__rebuild_index)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__redigest_item)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__resolve_view)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__restore_items)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__restore_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__retry_job)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__revoke_connector)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__save_draft)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__search_items)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__set_lifecycle)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__set_rules)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__set_type)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__start_import)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__unarchive_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__help__subcmd__update_workspace)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_connectors)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_drafts)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_events)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_items)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_jobs)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_proposals)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_reviews)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_tenant_events)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_tenant_jobs)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_types)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__list_workspaces)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__log_items)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__move_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__open_proposal)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__open_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__present_view)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__preview_names)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__purge_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__purge_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__read_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__rebuild_index)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__redigest_item)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__resolve_view)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__restore_items)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__restore_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__retry_job)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__revoke_connector)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__save_draft)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__search_items)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__set_lifecycle)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__set_rules)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__set_type)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__start_import)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__unarchive_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        okf__subcmd__jawn__subcmd__update_workspace)
            opts="-h --server --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --server)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --json)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _okf__jawn -o nosort -o bashdefault -o default okf-jawn
else
    complete -F _okf__jawn -o bashdefault -o default okf-jawn
fi
