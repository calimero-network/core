from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.policy_view_source import PolicyViewSource
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.upgrade_policy import UpgradePolicy


T = TypeVar("T", bound="PolicyView")


@_attrs_define
class PolicyView:
    """An update policy at one scope: the organization's (`app` absent) or an app's. `policy` is the one in force; `source`
    where it comes from (the app's own, its organization's, or the defaults); `own` the one set at this scope, `null`
    for none.

        Attributes:
            org (str):
            policy (UpgradePolicy): How an organization's owner and admins let an app's updates apply (`POST
                /orgs/{org}/upgrade-policy`, `POST /apps/{app}/upgrade-policy`); every field is answered; a request may leave
                any out (its default). `secondApprover` (default off): a red update needs two distinct owners or admins.
                `autoSealed` (default off) and `holdHours` (24 to 168, default 24): a green update of an app that stays Sealed
                (no destination outside the cluster before or after, members counted) applying on its own after the hold; stored
                and validated now, carried out by a later release.
            source (PolicyViewSource):
            own (Union['UpgradePolicy', None]):
            app (Union[Unset, str]):
    """

    org: str
    policy: "UpgradePolicy"
    source: PolicyViewSource
    own: Union["UpgradePolicy", None]
    app: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.upgrade_policy import UpgradePolicy

        org = self.org

        policy = self.policy.to_dict()

        source = self.source.value

        own: Union[None, dict[str, Any]]
        if isinstance(self.own, UpgradePolicy):
            own = self.own.to_dict()
        else:
            own = self.own

        app = self.app

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "policy": policy,
                "source": source,
                "own": own,
            }
        )
        if app is not UNSET:
            field_dict["app"] = app

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.upgrade_policy import UpgradePolicy

        d = dict(src_dict)
        org = d.pop("org")

        policy = UpgradePolicy.from_dict(d.pop("policy"))

        source = PolicyViewSource(d.pop("source"))

        def _parse_own(data: object) -> Union["UpgradePolicy", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                own_type_0 = UpgradePolicy.from_dict(data)

                return own_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UpgradePolicy", None], data)

        own = _parse_own(d.pop("own"))

        app = d.pop("app", UNSET)

        policy_view = cls(
            org=org,
            policy=policy,
            source=source,
            own=own,
            app=app,
        )

        policy_view.additional_properties = d
        return policy_view

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
