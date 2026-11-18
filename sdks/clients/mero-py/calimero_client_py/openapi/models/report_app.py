from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.report_app_releases_approved_by import ReportAppReleasesApprovedBy
from ..models.report_app_upgrades_held_by import ReportAppUpgradesHeldBy
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.report_app_publisher_type_0 import ReportAppPublisherType0
    from ..models.app_scope_type_0 import AppScopeType0
    from ..models.report_app_replicas_item import ReportAppReplicasItem
    from ..models.app_scope_type_1 import AppScopeType1
    from ..models.report_app_package_type_0 import ReportAppPackageType0
    from ..models.report_app_latest_blocked import ReportAppLatestBlocked
    from ..models.report_upgrade import ReportUpgrade


T = TypeVar("T", bound="ReportApp")


@_attrs_define
class ReportApp:
    """One app in a release report (`GET /auth/apps`, `GET /orgs/{org}/apps`, `GET /ops/apps`): its bundle and release
    (`package`, `null` where the reader may not see it), the publisher's key and bound organization, who holds its
    upgrades and release approvals, its last upgrade, the releases approved to hold it (`null`: unrestricted), each
    replica's release, and `lastWriteAt` (Unix seconds, `null` when unknown). `name`, `team`, `availableVersions` (newer
    releases, oldest first), `latestReachable` and `latestBlocked` (the newest release and why an upgrade would not take
    it) are for the app's people, never the operator.

        Attributes:
            app (str):
            group (int):
            scope (Union['AppScopeType0', 'AppScopeType1']): The scope whose approvals cover an app: its organization
                (`personal: true` when it is its owner's personal organization). `account` is only `GET /auth/apps`' own scope:
                the apps the account owns, wherever they are.
            bundle (str):
            package (Union['ReportAppPackageType0', None]):
            publisher (Union['ReportAppPublisherType0', None]):
            upgrades_held_by (ReportAppUpgradesHeldBy):
            releases_approved_by (ReportAppReleasesApprovedBy):
            last_upgrade (Union['ReportUpgrade', None]):
            approved_releases (Union[None, list[str]]):
            replicas (list['ReportAppReplicasItem']):
            last_write_at (Union[None, int]):
            name (Union[Unset, str]):
            team (Union[Unset, str]):
            available_versions (Union[Unset, list[str]]):
            latest_reachable (Union[None, Unset, str]):
            latest_blocked (Union[Unset, ReportAppLatestBlocked]):
    """

    app: str
    group: int
    scope: Union["AppScopeType0", "AppScopeType1"]
    bundle: str
    package: Union["ReportAppPackageType0", None]
    publisher: Union["ReportAppPublisherType0", None]
    upgrades_held_by: ReportAppUpgradesHeldBy
    releases_approved_by: ReportAppReleasesApprovedBy
    last_upgrade: Union["ReportUpgrade", None]
    approved_releases: Union[None, list[str]]
    replicas: list["ReportAppReplicasItem"]
    last_write_at: Union[None, int]
    name: Union[Unset, str] = UNSET
    team: Union[Unset, str] = UNSET
    available_versions: Union[Unset, list[str]] = UNSET
    latest_reachable: Union[None, Unset, str] = UNSET
    latest_blocked: Union[Unset, "ReportAppLatestBlocked"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.report_app_publisher_type_0 import ReportAppPublisherType0
        from ..models.app_scope_type_0 import AppScopeType0
        from ..models.report_app_replicas_item import ReportAppReplicasItem
        from ..models.app_scope_type_1 import AppScopeType1
        from ..models.report_app_package_type_0 import ReportAppPackageType0
        from ..models.report_app_latest_blocked import ReportAppLatestBlocked
        from ..models.report_upgrade import ReportUpgrade

        app = self.app

        group = self.group

        scope: dict[str, Any]
        if isinstance(self.scope, AppScopeType0):
            scope = self.scope.to_dict()
        else:
            scope = self.scope.to_dict()

        bundle = self.bundle

        package: Union[None, dict[str, Any]]
        if isinstance(self.package, ReportAppPackageType0):
            package = self.package.to_dict()
        else:
            package = self.package

        publisher: Union[None, dict[str, Any]]
        if isinstance(self.publisher, ReportAppPublisherType0):
            publisher = self.publisher.to_dict()
        else:
            publisher = self.publisher

        upgrades_held_by = self.upgrades_held_by.value

        releases_approved_by = self.releases_approved_by.value

        last_upgrade: Union[None, dict[str, Any]]
        if isinstance(self.last_upgrade, ReportUpgrade):
            last_upgrade = self.last_upgrade.to_dict()
        else:
            last_upgrade = self.last_upgrade

        approved_releases: Union[None, list[str]]
        if isinstance(self.approved_releases, list):
            approved_releases = self.approved_releases

        else:
            approved_releases = self.approved_releases

        replicas = []
        for replicas_item_data in self.replicas:
            replicas_item = replicas_item_data.to_dict()
            replicas.append(replicas_item)

        last_write_at: Union[None, int]
        last_write_at = self.last_write_at

        name = self.name

        team = self.team

        available_versions: Union[Unset, list[str]] = UNSET
        if not isinstance(self.available_versions, Unset):
            available_versions = self.available_versions

        latest_reachable: Union[None, Unset, str]
        if isinstance(self.latest_reachable, Unset):
            latest_reachable = UNSET
        else:
            latest_reachable = self.latest_reachable

        latest_blocked: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.latest_blocked, Unset):
            latest_blocked = self.latest_blocked.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "group": group,
                "scope": scope,
                "bundle": bundle,
                "package": package,
                "publisher": publisher,
                "upgradesHeldBy": upgrades_held_by,
                "releasesApprovedBy": releases_approved_by,
                "lastUpgrade": last_upgrade,
                "approvedReleases": approved_releases,
                "replicas": replicas,
                "lastWriteAt": last_write_at,
            }
        )
        if name is not UNSET:
            field_dict["name"] = name
        if team is not UNSET:
            field_dict["team"] = team
        if available_versions is not UNSET:
            field_dict["availableVersions"] = available_versions
        if latest_reachable is not UNSET:
            field_dict["latestReachable"] = latest_reachable
        if latest_blocked is not UNSET:
            field_dict["latestBlocked"] = latest_blocked

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.report_app_publisher_type_0 import ReportAppPublisherType0
        from ..models.app_scope_type_0 import AppScopeType0
        from ..models.report_app_replicas_item import ReportAppReplicasItem
        from ..models.app_scope_type_1 import AppScopeType1
        from ..models.report_app_package_type_0 import ReportAppPackageType0
        from ..models.report_app_latest_blocked import ReportAppLatestBlocked
        from ..models.report_upgrade import ReportUpgrade

        d = dict(src_dict)
        app = d.pop("app")

        group = d.pop("group")

        def _parse_scope(data: object) -> Union["AppScopeType0", "AppScopeType1"]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_app_scope_type_0 = AppScopeType0.from_dict(data)

                return componentsschemas_app_scope_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_app_scope_type_1 = AppScopeType1.from_dict(data)

            return componentsschemas_app_scope_type_1

        scope = _parse_scope(d.pop("scope"))

        bundle = d.pop("bundle")

        def _parse_package(data: object) -> Union["ReportAppPackageType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                package_type_0 = ReportAppPackageType0.from_dict(data)

                return package_type_0
            except:  # noqa: E722
                pass
            return cast(Union["ReportAppPackageType0", None], data)

        package = _parse_package(d.pop("package"))

        def _parse_publisher(data: object) -> Union["ReportAppPublisherType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                publisher_type_0 = ReportAppPublisherType0.from_dict(data)

                return publisher_type_0
            except:  # noqa: E722
                pass
            return cast(Union["ReportAppPublisherType0", None], data)

        publisher = _parse_publisher(d.pop("publisher"))

        upgrades_held_by = ReportAppUpgradesHeldBy(d.pop("upgradesHeldBy"))

        releases_approved_by = ReportAppReleasesApprovedBy(d.pop("releasesApprovedBy"))

        def _parse_last_upgrade(data: object) -> Union["ReportUpgrade", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                last_upgrade_type_0 = ReportUpgrade.from_dict(data)

                return last_upgrade_type_0
            except:  # noqa: E722
                pass
            return cast(Union["ReportUpgrade", None], data)

        last_upgrade = _parse_last_upgrade(d.pop("lastUpgrade"))

        def _parse_approved_releases(data: object) -> Union[None, list[str]]:
            if data is None:
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                approved_releases_type_0 = cast(list[str], data)

                return approved_releases_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, list[str]], data)

        approved_releases = _parse_approved_releases(d.pop("approvedReleases"))

        replicas = []
        _replicas = d.pop("replicas")
        for replicas_item_data in _replicas:
            replicas_item = ReportAppReplicasItem.from_dict(replicas_item_data)

            replicas.append(replicas_item)

        def _parse_last_write_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        last_write_at = _parse_last_write_at(d.pop("lastWriteAt"))

        name = d.pop("name", UNSET)

        team = d.pop("team", UNSET)

        available_versions = cast(list[str], d.pop("availableVersions", UNSET))

        def _parse_latest_reachable(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        latest_reachable = _parse_latest_reachable(d.pop("latestReachable", UNSET))

        _latest_blocked = d.pop("latestBlocked", UNSET)
        latest_blocked: Union[Unset, ReportAppLatestBlocked]
        if isinstance(_latest_blocked, Unset):
            latest_blocked = UNSET
        else:
            latest_blocked = ReportAppLatestBlocked.from_dict(_latest_blocked)

        report_app = cls(
            app=app,
            group=group,
            scope=scope,
            bundle=bundle,
            package=package,
            publisher=publisher,
            upgrades_held_by=upgrades_held_by,
            releases_approved_by=releases_approved_by,
            last_upgrade=last_upgrade,
            approved_releases=approved_releases,
            replicas=replicas,
            last_write_at=last_write_at,
            name=name,
            team=team,
            available_versions=available_versions,
            latest_reachable=latest_reachable,
            latest_blocked=latest_blocked,
        )

        report_app.additional_properties = d
        return report_app

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
