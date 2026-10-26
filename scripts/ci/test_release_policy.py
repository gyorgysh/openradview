# SPDX-License-Identifier: GPL-3.0-or-later
"""Fail-closed approval policy tests. No API writes, credentials or Apple calls."""
import copy
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import os
spec = importlib.util.spec_from_file_location('release_policy', Path(__file__).with_name('release-policy.py'))
policy = importlib.util.module_from_spec(spec); spec.loader.exec_module(policy)
class ApprovalTests(unittest.TestCase):
    def setUp(self):
        self.environment = {'protection_rules':[{'type':'required_reviewers','reviewers':[{'type':'User','reviewer':{'id':policy.OWNER}}]}], 'deployment_branch_policy':{'protected_branches':False,'custom_branch_policies':True}}
        self.policies = {'branch_policies':[{'name':'v*','type':'tag'}]}
    def test_real_reviewer_and_tag_restrictions_are_both_required(self):
        policy.protected(self.environment, self.policies)
        variants = [({}, self.policies)]
        wrong = copy.deepcopy(self.environment); wrong['protection_rules'][0]['reviewers'][0]['reviewer']['id'] = 1
        variants.append((wrong,self.policies))
        wrong = copy.deepcopy(self.environment); wrong['protection_rules'] = [{'type':'wait_timer'}]
        variants.append((wrong,self.policies))
        wrong = copy.deepcopy(self.environment); wrong['deployment_branch_policy'] = None
        variants.append((wrong,self.policies))
        for rules in [[],[{'name':'v*','type':'branch'}],[{'name':'*','type':'tag'}],[{'name':'v*','type':'tag'},{'name':'main','type':'branch'}]]:
            variants.append((self.environment,{'branch_policies':rules}))
        for environment, policies in variants:
            with self.subTest(environment=environment,policies=policies), self.assertRaises(ValueError): policy.protected(environment,policies)
    def test_private_manual_mode_keeps_tag_restrictions_without_claiming_a_reviewer(self):
        environment = copy.deepcopy(self.environment); environment['protection_rules'] = []
        policy.protected(environment,self.policies,require_reviewer=False)
        with self.assertRaises(ValueError): policy.protected(environment,self.policies)
        with self.assertRaises(ValueError): policy.protected(environment,{'branch_policies':[]},require_reviewer=False)
    def test_private_execution_requires_owner_dispatch_and_public_requires_reviewers(self):
        environment = copy.deepcopy(self.environment)
        environment['protection_rules'] = []
        private = True
        def response(path):
            return {'':{'private':private},'environments/release':environment,'environments/release/deployment-branch-policies':self.policies,'immutable-releases':{'enabled':True}}[path]
        with patch.object(policy,'api',side_effect=response):
            for event, actor, allowed in [('push',str(policy.OWNER),False),('workflow_dispatch','1',False),('workflow_dispatch',str(policy.OWNER),True)]:
                with self.subTest(event=event,actor=actor), patch.dict(os.environ,{'GITHUB_EVENT_NAME':event,'GITHUB_ACTOR_ID':actor,'ORV_RELEASE_APPROVAL_READY':policy.MANUAL},clear=True):
                    if allowed: self.assertTrue(policy.check_environment(executing=True))
                    else:
                        with self.assertRaises(ValueError): policy.check_environment(executing=True)
            private = False
            with self.assertRaises(ValueError): policy.check_environment(executing=True)
            environment.update(self.environment)
            with patch.dict(os.environ,{'ORV_RELEASE_APPROVAL_READY':policy.READY}): self.assertFalse(policy.check_environment(executing=True))
if __name__ == '__main__': unittest.main()
