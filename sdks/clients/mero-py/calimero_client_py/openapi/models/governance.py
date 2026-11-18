from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.governance_app_upgrades import GovernanceAppUpgrades
from ..models.governance_deployers import GovernanceDeployers
from ..models.governance_deployments import GovernanceDeployments
from ..models.governance_outbound import GovernanceOutbound
from ..models.governance_releases import GovernanceReleases
from ..models.governance_sign_up import GovernanceSignUp
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="Governance")


@_attrs_define
class Governance:
    """Who decides what on this cluster: release approvals, app upgrades and outbound approvals (`owner`: each app's owner;
    `operator`: the operator), and, left out at their defaults, `deployments` (`open`), `deployers` (`operator`) and
    `signUp` (`open`).

        Attributes:
            releases (GovernanceReleases):
            app_upgrades (GovernanceAppUpgrades):
            outbound (GovernanceOutbound):
            deployments (Union[Unset, GovernanceDeployments]):
            deployers (Union[Unset, GovernanceDeployers]):
            sign_up (Union[Unset, GovernanceSignUp]):
    """

    releases: GovernanceReleases
    app_upgrades: GovernanceAppUpgrades
    outbound: GovernanceOutbound
    deployments: Union[Unset, GovernanceDeployments] = UNSET
    deployers: Union[Unset, GovernanceDeployers] = UNSET
    sign_up: Union[Unset, GovernanceSignUp] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        releases = self.releases.value

        app_upgrades = self.app_upgrades.value

        outbound = self.outbound.value

        deployments: Union[Unset, str] = UNSET
        if not isinstance(self.deployments, Unset):
            deployments = self.deployments.value

        deployers: Union[Unset, str] = UNSET
        if not isinstance(self.deployers, Unset):
            deployers = self.deployers.value

        sign_up: Union[Unset, str] = UNSET
        if not isinstance(self.sign_up, Unset):
            sign_up = self.sign_up.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "releases": releases,
                "appUpgrades": app_upgrades,
                "outbound": outbound,
            }
        )
        if deployments is not UNSET:
            field_dict["deployments"] = deployments
        if deployers is not UNSET:
            field_dict["deployers"] = deployers
        if sign_up is not UNSET:
            field_dict["signUp"] = sign_up

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        releases = GovernanceReleases(d.pop("releases"))

        app_upgrades = GovernanceAppUpgrades(d.pop("appUpgrades"))

        outbound = GovernanceOutbound(d.pop("outbound"))

        _deployments = d.pop("deployments", UNSET)
        deployments: Union[Unset, GovernanceDeployments]
        if isinstance(_deployments, Unset):
            deployments = UNSET
        else:
            deployments = GovernanceDeployments(_deployments)

        _deployers = d.pop("deployers", UNSET)
        deployers: Union[Unset, GovernanceDeployers]
        if isinstance(_deployers, Unset):
            deployers = UNSET
        else:
            deployers = GovernanceDeployers(_deployers)

        _sign_up = d.pop("signUp", UNSET)
        sign_up: Union[Unset, GovernanceSignUp]
        if isinstance(_sign_up, Unset):
            sign_up = UNSET
        else:
            sign_up = GovernanceSignUp(_sign_up)

        governance = cls(
            releases=releases,
            app_upgrades=app_upgrades,
            outbound=outbound,
            deployments=deployments,
            deployers=deployers,
            sign_up=sign_up,
        )

        governance.additional_properties = d
        return governance

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
