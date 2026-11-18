from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.report_upgrade_actor_kind import ReportUpgradeActorKind
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.bundle_release import BundleRelease


T = TypeVar("T", bound="ReportUpgrade")


@_attrs_define
class ReportUpgrade:
    """An app's last upgrade as a report shows it. `actor`, `error`, `run` and `apiKey` are shown to the app's people,
    never to the operator.

        Attributes:
            from_ (BundleRelease): A bundle by its digest, with the release it came from where the reader may see it.
            to (BundleRelease): A bundle by its digest, with the release it came from where the reader may see it.
            at (int):
            ok (bool):
            actor_kind (ReportUpgradeActorKind):
            actor (Union[Unset, str]):
            error (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
                it is).
            run (Union[Unset, str]):
            api_key (Union[Unset, str]):
    """

    from_: "BundleRelease"
    to: "BundleRelease"
    at: int
    ok: bool
    actor_kind: ReportUpgradeActorKind
    actor: Union[Unset, str] = UNSET
    error: Union[Unset, Any] = UNSET
    run: Union[Unset, str] = UNSET
    api_key: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.bundle_release import BundleRelease

        from_ = self.from_.to_dict()

        to = self.to.to_dict()

        at = self.at

        ok = self.ok

        actor_kind = self.actor_kind.value

        actor = self.actor

        error = self.error

        run = self.run

        api_key = self.api_key

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "from": from_,
                "to": to,
                "at": at,
                "ok": ok,
                "actorKind": actor_kind,
            }
        )
        if actor is not UNSET:
            field_dict["actor"] = actor
        if error is not UNSET:
            field_dict["error"] = error
        if run is not UNSET:
            field_dict["run"] = run
        if api_key is not UNSET:
            field_dict["apiKey"] = api_key

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.bundle_release import BundleRelease

        d = dict(src_dict)
        from_ = BundleRelease.from_dict(d.pop("from"))

        to = BundleRelease.from_dict(d.pop("to"))

        at = d.pop("at")

        ok = d.pop("ok")

        actor_kind = ReportUpgradeActorKind(d.pop("actorKind"))

        actor = d.pop("actor", UNSET)

        error = d.pop("error", UNSET)

        run = d.pop("run", UNSET)

        api_key = d.pop("apiKey", UNSET)

        report_upgrade = cls(
            from_=from_,
            to=to,
            at=at,
            ok=ok,
            actor_kind=actor_kind,
            actor=actor,
            error=error,
            run=run,
            api_key=api_key,
        )

        report_upgrade.additional_properties = d
        return report_upgrade

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
