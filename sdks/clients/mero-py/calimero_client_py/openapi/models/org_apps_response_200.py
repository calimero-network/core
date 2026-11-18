from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.app_scope_type_0 import AppScopeType0
    from ..models.app_scope_type_1 import AppScopeType1
    from ..models.governance import Governance
    from ..models.cluster_release import ClusterRelease
    from ..models.report_app import ReportApp


T = TypeVar("T", bound="OrgAppsResponse200")


@_attrs_define
class OrgAppsResponse200:
    """
    Attributes:
        scope (Union['AppScopeType0', 'AppScopeType1']): The scope whose approvals cover an app: its organization
            (`personal: true` when it is its owner's personal organization). `account` is only `GET /auth/apps`' own scope:
            the apps the account owns, wherever they are.
        governance (Governance): Who decides what on this cluster: release approvals, app upgrades and outbound
            approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults, `deployments`
            (`open`), `deployers` (`operator`) and `signUp` (`open`).
        approved_releases (Union[None, list[str]]):
        cluster_releases (list['ClusterRelease']):
        apps (list['ReportApp']):
        next_ (Union[None, str]):
    """

    scope: Union["AppScopeType0", "AppScopeType1"]
    governance: "Governance"
    approved_releases: Union[None, list[str]]
    cluster_releases: list["ClusterRelease"]
    apps: list["ReportApp"]
    next_: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_scope_type_0 import AppScopeType0
        from ..models.app_scope_type_1 import AppScopeType1
        from ..models.governance import Governance
        from ..models.cluster_release import ClusterRelease
        from ..models.report_app import ReportApp

        scope: dict[str, Any]
        if isinstance(self.scope, AppScopeType0):
            scope = self.scope.to_dict()
        else:
            scope = self.scope.to_dict()

        governance = self.governance.to_dict()

        approved_releases: Union[None, list[str]]
        if isinstance(self.approved_releases, list):
            approved_releases = self.approved_releases

        else:
            approved_releases = self.approved_releases

        cluster_releases = []
        for cluster_releases_item_data in self.cluster_releases:
            cluster_releases_item = cluster_releases_item_data.to_dict()
            cluster_releases.append(cluster_releases_item)

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        next_: Union[None, str]
        next_ = self.next_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "scope": scope,
                "governance": governance,
                "approvedReleases": approved_releases,
                "clusterReleases": cluster_releases,
                "apps": apps,
                "next": next_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_scope_type_0 import AppScopeType0
        from ..models.app_scope_type_1 import AppScopeType1
        from ..models.governance import Governance
        from ..models.cluster_release import ClusterRelease
        from ..models.report_app import ReportApp

        d = dict(src_dict)

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

        governance = Governance.from_dict(d.pop("governance"))

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

        cluster_releases = []
        _cluster_releases = d.pop("clusterReleases")
        for cluster_releases_item_data in _cluster_releases:
            cluster_releases_item = ClusterRelease.from_dict(cluster_releases_item_data)

            cluster_releases.append(cluster_releases_item)

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = ReportApp.from_dict(apps_item_data)

            apps.append(apps_item)

        def _parse_next_(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        next_ = _parse_next_(d.pop("next"))

        org_apps_response_200 = cls(
            scope=scope,
            governance=governance,
            approved_releases=approved_releases,
            cluster_releases=cluster_releases,
            apps=apps,
            next_=next_,
        )

        org_apps_response_200.additional_properties = d
        return org_apps_response_200

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
