from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_mode import OrgMode
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.org_upgrade_runs import OrgUpgradeRuns
    from ..models.org_new_work_stopped_type_0 import OrgNewWorkStoppedType0
    from ..models.org_member_metadata import OrgMemberMetadata
    from ..models.org_teams import OrgTeams
    from ..models.org_capabilities import OrgCapabilities
    from ..models.org_members import OrgMembers
    from ..models.org_outbound_approvals import OrgOutboundApprovals
    from ..models.org_recovery import OrgRecovery


T = TypeVar("T", bound="Org")


@_attrs_define
class Org:
    """An organization as `GET /orgs/{org}` answers it. `members` maps each account to its role, `capabilities` and
    `defaultCapabilities` are capability bits, `outboundApprovals` is keyed by package signer, `upgradeRuns` by run id;
    `recovery` is absent when the organization set no recovery key, `allApps` when no member gets every app added later,
    `personal` unless it is its owner's personal organization (where instances created without naming an organization
    go).

        Attributes:
            org_id (str):
            name (str):
            members (OrgMembers):
            teams (OrgTeams):
            default_capabilities (int):
            capabilities (OrgCapabilities):
            approved_releases (list[str]):
            outbound_approvals (OrgOutboundApprovals):
            metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            member_metadata (OrgMemberMetadata):
            upgrade_runs (OrgUpgradeRuns):
            recovery (Union[Unset, OrgRecovery]):
            all_apps (Union[Unset, list[str]]):
            personal (Union[Unset, bool]):
            mode (Union[Unset, OrgMode]):
            release_approvers (Union[Unset, list[str]]):
            publishes (Union[Unset, list[str]]):
            new_work_stopped (Union['OrgNewWorkStoppedType0', None, Unset]):
    """

    org_id: str
    name: str
    members: "OrgMembers"
    teams: "OrgTeams"
    default_capabilities: int
    capabilities: "OrgCapabilities"
    approved_releases: list[str]
    outbound_approvals: "OrgOutboundApprovals"
    metadata: Any
    member_metadata: "OrgMemberMetadata"
    upgrade_runs: "OrgUpgradeRuns"
    recovery: Union[Unset, "OrgRecovery"] = UNSET
    all_apps: Union[Unset, list[str]] = UNSET
    personal: Union[Unset, bool] = UNSET
    mode: Union[Unset, OrgMode] = UNSET
    release_approvers: Union[Unset, list[str]] = UNSET
    publishes: Union[Unset, list[str]] = UNSET
    new_work_stopped: Union["OrgNewWorkStoppedType0", None, Unset] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.org_upgrade_runs import OrgUpgradeRuns
        from ..models.org_new_work_stopped_type_0 import OrgNewWorkStoppedType0
        from ..models.org_member_metadata import OrgMemberMetadata
        from ..models.org_teams import OrgTeams
        from ..models.org_capabilities import OrgCapabilities
        from ..models.org_members import OrgMembers
        from ..models.org_outbound_approvals import OrgOutboundApprovals
        from ..models.org_recovery import OrgRecovery

        org_id = self.org_id

        name = self.name

        members = self.members.to_dict()

        teams = self.teams.to_dict()

        default_capabilities = self.default_capabilities

        capabilities = self.capabilities.to_dict()

        approved_releases = self.approved_releases

        outbound_approvals = self.outbound_approvals.to_dict()

        metadata = self.metadata

        member_metadata = self.member_metadata.to_dict()

        upgrade_runs = self.upgrade_runs.to_dict()

        recovery: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.recovery, Unset):
            recovery = self.recovery.to_dict()

        all_apps: Union[Unset, list[str]] = UNSET
        if not isinstance(self.all_apps, Unset):
            all_apps = self.all_apps

        personal = self.personal

        mode: Union[Unset, str] = UNSET
        if not isinstance(self.mode, Unset):
            mode = self.mode.value

        release_approvers: Union[Unset, list[str]] = UNSET
        if not isinstance(self.release_approvers, Unset):
            release_approvers = self.release_approvers

        publishes: Union[Unset, list[str]] = UNSET
        if not isinstance(self.publishes, Unset):
            publishes = self.publishes

        new_work_stopped: Union[None, Unset, dict[str, Any]]
        if isinstance(self.new_work_stopped, Unset):
            new_work_stopped = UNSET
        elif isinstance(self.new_work_stopped, OrgNewWorkStoppedType0):
            new_work_stopped = self.new_work_stopped.to_dict()
        else:
            new_work_stopped = self.new_work_stopped

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "orgId": org_id,
                "name": name,
                "members": members,
                "teams": teams,
                "defaultCapabilities": default_capabilities,
                "capabilities": capabilities,
                "approvedReleases": approved_releases,
                "outboundApprovals": outbound_approvals,
                "metadata": metadata,
                "memberMetadata": member_metadata,
                "upgradeRuns": upgrade_runs,
            }
        )
        if recovery is not UNSET:
            field_dict["recovery"] = recovery
        if all_apps is not UNSET:
            field_dict["allApps"] = all_apps
        if personal is not UNSET:
            field_dict["personal"] = personal
        if mode is not UNSET:
            field_dict["mode"] = mode
        if release_approvers is not UNSET:
            field_dict["releaseApprovers"] = release_approvers
        if publishes is not UNSET:
            field_dict["publishes"] = publishes
        if new_work_stopped is not UNSET:
            field_dict["newWorkStopped"] = new_work_stopped

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.org_upgrade_runs import OrgUpgradeRuns
        from ..models.org_new_work_stopped_type_0 import OrgNewWorkStoppedType0
        from ..models.org_member_metadata import OrgMemberMetadata
        from ..models.org_teams import OrgTeams
        from ..models.org_capabilities import OrgCapabilities
        from ..models.org_members import OrgMembers
        from ..models.org_outbound_approvals import OrgOutboundApprovals
        from ..models.org_recovery import OrgRecovery

        d = dict(src_dict)
        org_id = d.pop("orgId")

        name = d.pop("name")

        members = OrgMembers.from_dict(d.pop("members"))

        teams = OrgTeams.from_dict(d.pop("teams"))

        default_capabilities = d.pop("defaultCapabilities")

        capabilities = OrgCapabilities.from_dict(d.pop("capabilities"))

        approved_releases = cast(list[str], d.pop("approvedReleases"))

        outbound_approvals = OrgOutboundApprovals.from_dict(d.pop("outboundApprovals"))

        metadata = d.pop("metadata")

        member_metadata = OrgMemberMetadata.from_dict(d.pop("memberMetadata"))

        upgrade_runs = OrgUpgradeRuns.from_dict(d.pop("upgradeRuns"))

        _recovery = d.pop("recovery", UNSET)
        recovery: Union[Unset, OrgRecovery]
        if isinstance(_recovery, Unset):
            recovery = UNSET
        else:
            recovery = OrgRecovery.from_dict(_recovery)

        all_apps = cast(list[str], d.pop("allApps", UNSET))

        personal = d.pop("personal", UNSET)

        _mode = d.pop("mode", UNSET)
        mode: Union[Unset, OrgMode]
        if isinstance(_mode, Unset):
            mode = UNSET
        else:
            mode = OrgMode(_mode)

        release_approvers = cast(list[str], d.pop("releaseApprovers", UNSET))

        publishes = cast(list[str], d.pop("publishes", UNSET))

        def _parse_new_work_stopped(
            data: object,
        ) -> Union["OrgNewWorkStoppedType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                new_work_stopped_type_0 = OrgNewWorkStoppedType0.from_dict(data)

                return new_work_stopped_type_0
            except:  # noqa: E722
                pass
            return cast(Union["OrgNewWorkStoppedType0", None, Unset], data)

        new_work_stopped = _parse_new_work_stopped(d.pop("newWorkStopped", UNSET))

        org = cls(
            org_id=org_id,
            name=name,
            members=members,
            teams=teams,
            default_capabilities=default_capabilities,
            capabilities=capabilities,
            approved_releases=approved_releases,
            outbound_approvals=outbound_approvals,
            metadata=metadata,
            member_metadata=member_metadata,
            upgrade_runs=upgrade_runs,
            recovery=recovery,
            all_apps=all_apps,
            personal=personal,
            mode=mode,
            release_approvers=release_approvers,
            publishes=publishes,
            new_work_stopped=new_work_stopped,
        )

        org.additional_properties = d
        return org

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
