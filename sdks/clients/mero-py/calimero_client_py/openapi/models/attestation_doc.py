from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.kms_keys import KmsKeys
    from ..models.governance import Governance
    from ..models.release import Release


T = TypeVar("T", bound="AttestationDoc")


@_attrs_define
class AttestationDoc:
    """A node's attestation: the TDX quote (base64), the key it binds (hex) and what it was asked for. `receiptKey` and
    `governance` with `purpose=receipts`, `kms` with `purpose=kms` on a node with a KMS record, `clusterIdentity` with
    `purpose=orchestrator`; `jobKey` only from a job node, never from this API.

        Attributes:
            quote (str):
            transport_key (str):
            app_hash (str):
            nonce (str):
            release (Release): A mero-tee release.
            cluster (str):
            node (int):
            mock (bool):
            receipt_key (Union[Unset, str]):
            governance (Union[Unset, Governance]): Who decides what on this cluster: release approvals, app upgrades and
                outbound approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults,
                `deployments` (`open`), `deployers` (`operator`) and `signUp` (`open`).
            kms (Union[Unset, KmsKeys]): What a node verified of the mero-kms replicas it took its keys from: the storage
                key `merod` fetched, and the data disk's key the boot script fetched; each absent without a record.
            job_key (Union[Unset, str]):
            cluster_identity (Union[Unset, str]):
    """

    quote: str
    transport_key: str
    app_hash: str
    nonce: str
    release: "Release"
    cluster: str
    node: int
    mock: bool
    receipt_key: Union[Unset, str] = UNSET
    governance: Union[Unset, "Governance"] = UNSET
    kms: Union[Unset, "KmsKeys"] = UNSET
    job_key: Union[Unset, str] = UNSET
    cluster_identity: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.kms_keys import KmsKeys
        from ..models.governance import Governance
        from ..models.release import Release

        quote = self.quote

        transport_key = self.transport_key

        app_hash = self.app_hash

        nonce = self.nonce

        release = self.release.to_dict()

        cluster = self.cluster

        node = self.node

        mock = self.mock

        receipt_key = self.receipt_key

        governance: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.governance, Unset):
            governance = self.governance.to_dict()

        kms: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.kms, Unset):
            kms = self.kms.to_dict()

        job_key = self.job_key

        cluster_identity = self.cluster_identity

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "quote": quote,
                "transportKey": transport_key,
                "appHash": app_hash,
                "nonce": nonce,
                "release": release,
                "cluster": cluster,
                "node": node,
                "mock": mock,
            }
        )
        if receipt_key is not UNSET:
            field_dict["receiptKey"] = receipt_key
        if governance is not UNSET:
            field_dict["governance"] = governance
        if kms is not UNSET:
            field_dict["kms"] = kms
        if job_key is not UNSET:
            field_dict["jobKey"] = job_key
        if cluster_identity is not UNSET:
            field_dict["clusterIdentity"] = cluster_identity

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.kms_keys import KmsKeys
        from ..models.governance import Governance
        from ..models.release import Release

        d = dict(src_dict)
        quote = d.pop("quote")

        transport_key = d.pop("transportKey")

        app_hash = d.pop("appHash")

        nonce = d.pop("nonce")

        release = Release.from_dict(d.pop("release"))

        cluster = d.pop("cluster")

        node = d.pop("node")

        mock = d.pop("mock")

        receipt_key = d.pop("receiptKey", UNSET)

        _governance = d.pop("governance", UNSET)
        governance: Union[Unset, Governance]
        if isinstance(_governance, Unset):
            governance = UNSET
        else:
            governance = Governance.from_dict(_governance)

        _kms = d.pop("kms", UNSET)
        kms: Union[Unset, KmsKeys]
        if isinstance(_kms, Unset):
            kms = UNSET
        else:
            kms = KmsKeys.from_dict(_kms)

        job_key = d.pop("jobKey", UNSET)

        cluster_identity = d.pop("clusterIdentity", UNSET)

        attestation_doc = cls(
            quote=quote,
            transport_key=transport_key,
            app_hash=app_hash,
            nonce=nonce,
            release=release,
            cluster=cluster,
            node=node,
            mock=mock,
            receipt_key=receipt_key,
            governance=governance,
            kms=kms,
            job_key=job_key,
            cluster_identity=cluster_identity,
        )

        attestation_doc.additional_properties = d
        return attestation_doc

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
