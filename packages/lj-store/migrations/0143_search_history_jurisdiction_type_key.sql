UPDATE user_search_history
SET filters = (filters - 'juridictionType' - 'juridiction_type')
              || jsonb_build_object(
                     'jurisdictionType',
                     COALESCE(filters -> 'juridictionType', filters -> 'juridiction_type')
                 )
WHERE filters ?| ARRAY['juridictionType', 'juridiction_type'];
